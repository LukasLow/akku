//! Embedded durable key-value store backed by an append-only Write-Ahead Log (WAL)
//! with IEEE 802.3 CRC32 framing, crash-safe truncated-tail recovery, and atomic
//! snapshot compaction via [`akku::fs::write_atomic`](crate::fs::write_atomic).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::fs::write_atomic;
use crate::time::{Clock, OffsetDateTime, SignedDuration, SystemClock};

use super::{BatchOp, KvError, KvLimits, KvResult, KvStore};

/// 8-byte WAL magic header (`"AKKUWAL\x01"`).
pub const WAL_MAGIC: &[u8; 8] = b"AKKUWAL\x01";

const OP_SET: u8 = 1;
const OP_DELETE: u8 = 2;
/// Batch envelope start marker; `expires_ms` carries the number of enclosed records.
const OP_BATCH_BEGIN: u8 = 3;
/// Batch envelope commit marker; `expires_ms` repeats the number of enclosed records.
const OP_BATCH_COMMIT: u8 = 4;
const NO_EXPIRY: i64 = 0;

/// Fixed frame header size: `crc32 (4) + op_tag (1) + expires_ms (8) + key_len (4) + val_len (4) = 21` bytes.
const FRAME_HEADER_LEN: usize = 21;

/// Precomputed IEEE 802.3 CRC32 lookup table (polynomial `0xEDB8_8320`).
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

/// Computes the IEEE 802.3 CRC32 checksum over `chunks`.
fn compute_crc32(chunks: &[&[u8]]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for chunk in chunks {
        for &byte in *chunk {
            let idx = ((crc ^ u32::from(byte)) & 0xFF) as usize;
            crc = (crc >> 8) ^ CRC32_TABLE[idx];
        }
    }
    !crc
}

/// Hard upper bound on a single WAL record body (`key_len + val_len`) accepted during
/// replay (1 GiB), independent of the configured [`KvLimits`]. Bounds allocations made from
/// on-disk length prefixes in addition to the remaining file size.
pub const WAL_MAX_RECORD_LEN: u64 = 1 << 30;

/// Extra slack (1 MiB) allowed on top of two maximal records when scanning a garbage tail.
const TAIL_SCAN_SLACK: u64 = 1024 * 1024;

/// Maximum number of payload bytes CRC-checked while resynchronising over a garbage tail.
const TAIL_SCAN_CRC_BUDGET: u64 = 256 * 1024 * 1024;

/// Decoded and structurally validated fixed-size frame header.
struct FrameHeader {
    crc: u32,
    op_tag: u8,
    expires_ms: i64,
    key_len: usize,
    val_len: usize,
}

/// Structurally validates a frame header whose body may use at most `body_available` bytes.
fn parse_frame_header(
    hdr: &[u8; FRAME_HEADER_LEN],
    body_available: u64,
) -> Result<FrameHeader, &'static str> {
    let crc = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    let op_tag = hdr[4];
    let mut exp = [0u8; 8];
    exp.copy_from_slice(&hdr[5..13]);
    let expires_ms = i64::from_le_bytes(exp);
    let key_len = u64::from(u32::from_le_bytes([hdr[13], hdr[14], hdr[15], hdr[16]]));
    let val_len = u64::from(u32::from_le_bytes([hdr[17], hdr[18], hdr[19], hdr[20]]));

    match op_tag {
        OP_SET | OP_DELETE => {
            if key_len == 0 {
                return Err("zero key length");
            }
            if op_tag == OP_DELETE && val_len != 0 {
                return Err("delete record with non-empty value");
            }
        }
        OP_BATCH_BEGIN | OP_BATCH_COMMIT => {
            if key_len != 0 || val_len != 0 {
                return Err("batch marker with non-empty body");
            }
            if expires_ms < 1 {
                return Err("batch marker with invalid record count");
            }
        }
        _ => return Err("unknown operation tag"),
    }
    let body = key_len + val_len;
    if body > WAL_MAX_RECORD_LEN {
        return Err("record length exceeds WAL_MAX_RECORD_LEN");
    }
    if body > body_available {
        return Err("record extends past end of file");
    }
    Ok(FrameHeader {
        crc,
        op_tag,
        expires_ms,
        key_len: key_len as usize,
        val_len: val_len as usize,
    })
}

/// Returns the total length of the complete, CRC-valid frame starting at `buf[pos..]`, or
/// `Ok(None)` if there is none. Every examined frame (header plus body) is charged to
/// `budget`; `Err(())` means the work budget ran out.
fn valid_frame_at(buf: &[u8], pos: usize, budget: &mut u64) -> Result<Option<usize>, ()> {
    let rest = &buf[pos..];
    if rest.len() < FRAME_HEADER_LEN {
        return Ok(None);
    }
    let mut hdr = [0u8; FRAME_HEADER_LEN];
    hdr.copy_from_slice(&rest[..FRAME_HEADER_LEN]);
    let available = (rest.len() - FRAME_HEADER_LEN) as u64;
    let Ok(h) = parse_frame_header(&hdr, available) else {
        return Ok(None);
    };
    let body_len = h.key_len + h.val_len;
    let cost = (FRAME_HEADER_LEN + body_len) as u64;
    if cost > *budget {
        return Err(());
    }
    *budget -= cost;
    let body = &rest[FRAME_HEADER_LEN..FRAME_HEADER_LEN + body_len];
    if compute_crc32(&[&hdr[4..], body]) == h.crc {
        Ok(Some(FRAME_HEADER_LEN + body_len))
    } else {
        Ok(None)
    }
}

/// Searches `buf` for the first offset at which a chain of complete, CRC-valid frames starts
/// that runs exactly to the end of `buf` (or to an all-zero remainder).
///
/// Requiring the whole chain to reach the end avoids mistaking a frame that happens to be
/// embedded in the value bytes of a torn final record for genuine data following a corrupt
/// record. Returns `None` if there is no such chain, `Some(Ok(pos))` if one is found, and
/// `Some(Err(()))` if the CRC work budget ran out before the scan finished.
fn find_valid_frame(buf: &[u8]) -> Option<Result<usize, ()>> {
    let mut budget = TAIL_SCAN_CRC_BUDGET;
    // Everything from `zero_from` onward is zero bytes (a preallocated tail).
    let zero_from = buf.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    let mut pos = 0usize;
    while buf.len().saturating_sub(pos) >= FRAME_HEADER_LEN {
        match valid_frame_at(buf, pos, &mut budget) {
            Err(()) => return Some(Err(())),
            Ok(None) => {}
            Ok(Some(len)) => {
                let mut end = pos + len;
                loop {
                    if end >= zero_from {
                        return Some(Ok(pos));
                    }
                    match valid_frame_at(buf, end, &mut budget) {
                        Err(()) => return Some(Err(())),
                        Ok(Some(next)) => end += next,
                        Ok(None) => break,
                    }
                }
            }
        }
        pos += 1;
    }
    None
}

/// Records of a batch envelope seen during replay whose commit marker has not been read yet.
struct PendingBatch {
    /// WAL offset of the `OP_BATCH_BEGIN` marker.
    start: u64,
    /// Record count announced by the begin marker.
    expected: i64,
    /// Buffered `(op_tag, key, entry)` records.
    records: Vec<(u8, String, FileEntry)>,
}

/// Applies one replayed `OP_SET` / `OP_DELETE` record to the in-memory index.
fn apply_replayed(
    index: &mut BTreeMap<String, FileEntry>,
    dead_records: &mut usize,
    op_tag: u8,
    key: String,
    entry: FileEntry,
    now_ms: i64,
) {
    if op_tag == OP_SET {
        if entry.is_expired(now_ms) {
            if index.remove(&key).is_some() {
                *dead_records += 1;
            }
            *dead_records += 1;
        } else if index.insert(key, entry).is_some() {
            *dead_records += 1;
        }
    } else if index.remove(&key).is_some() {
        *dead_records += 2;
    } else {
        *dead_records += 1;
    }
}

/// Returns the path of the advisory lock file guarding the WAL at `path` (`<path>.lock`).
pub fn lock_path(path: impl AsRef<Path>) -> PathBuf {
    let mut name: OsString = path.as_ref().as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// Exclusive ownership of a WAL, held via a `create_new` lock file removed on drop.
#[derive(Debug)]
struct LockFile {
    path: PathBuf,
}

impl LockFile {
    fn acquire(store_path: &Path) -> KvResult<Self> {
        let path = lock_path(store_path);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let lock = Self { path };
                // The PID is informational only (shown when a second open is rejected).
                file.write_all(format!("{}\n", std::process::id()).as_bytes())?;
                Ok(lock)
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                let mut owner = String::new();
                if let Ok(f) = File::open(&path) {
                    let _ = f.take(32).read_to_string(&mut owner);
                }
                let owner = owner.trim();
                let owner = if owner.is_empty() { "unknown" } else { owner };
                Err(KvError::Io(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "FileStore '{}' is locked by lock file '{}' (owner pid {owner}); \
                         if no process still uses the store, remove it with \
                         FileStore::break_lock",
                        store_path.display(),
                        path.display()
                    ),
                )))
            }
            Err(err) => Err(KvError::Io(err)),
        }
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone)]
struct FileEntry {
    value: Vec<u8>,
    expires_ms: i64,
}

impl FileEntry {
    fn is_expired(&self, now_ms: i64) -> bool {
        self.expires_ms != NO_EXPIRY && now_ms >= self.expires_ms
    }
}

fn datetime_to_unix_ms(dt: OffsetDateTime) -> i64 {
    dt.unix_timestamp()
        .saturating_mul(1000)
        .saturating_add(i64::from(dt.time().nanosecond() / 1_000_000))
}

fn compute_expires_ms(clock: &dyn Clock, ttl: Duration) -> i64 {
    if ttl.is_zero() {
        return i64::MIN;
    }
    let akku_ttl = SignedDuration::try_from(ttl).unwrap_or(SignedDuration::MAX);
    let target = clock
        .now()
        .checked_add(akku_ttl)
        .unwrap_or_else(|| clock.now());
    let ms = datetime_to_unix_ms(target);
    if ms == NO_EXPIRY {
        1
    } else {
        ms
    }
}

fn encode_record(op_tag: u8, expires_ms: i64, key: &[u8], value: &[u8], out: &mut Vec<u8>) {
    let expires_bytes = expires_ms.to_le_bytes();
    let key_len_bytes = (key.len() as u32).to_le_bytes();
    let val_len_bytes = (value.len() as u32).to_le_bytes();
    let op_slice = [op_tag];

    let crc = compute_crc32(&[
        &op_slice,
        &expires_bytes,
        &key_len_bytes,
        &val_len_bytes,
        key,
        value,
    ]);

    out.reserve(FRAME_HEADER_LEN + key.len() + value.len());
    out.extend_from_slice(&crc.to_le_bytes());
    out.push(op_tag);
    out.extend_from_slice(&expires_bytes);
    out.extend_from_slice(&key_len_bytes);
    out.extend_from_slice(&val_len_bytes);
    out.extend_from_slice(key);
    out.extend_from_slice(value);
}

fn validate_store_path(path: &Path) -> KvResult<()> {
    let s = path.as_os_str().to_string_lossy();
    if s.is_empty() {
        return Err(KvError::InvalidInput(
            "FileStore path must not be empty".to_string(),
        ));
    }
    if s.contains('\0') {
        return Err(KvError::InvalidInput(
            "FileStore path must not contain NUL bytes".to_string(),
        ));
    }
    if path.is_dir() {
        return Err(KvError::InvalidInput(format!(
            "FileStore path '{}' is a directory",
            path.display()
        )));
    }
    Ok(())
}

struct FileStoreInner {
    file: File,
    /// Length of the WAL up to the last fully appended record.
    wal_len: u64,
    index: BTreeMap<String, FileEntry>,
    dead_records: usize,
    auto_compact_threshold: Option<usize>,
    limits: KvLimits,
    /// Set when the WAL handle could not be re-established after compaction; every later
    /// write fails with this message instead of appending to a stale (unlinked) file.
    poisoned: Option<String>,
}

/// An embedded, crash-safe, append-only WAL key-value store with CRC32 framing and atomic compaction.
///
/// # On-Disk Format
/// - 8-byte file magic: `b"AKKUWAL\x01"`.
/// - Little-endian framed records:
///   `[crc32: u32][op_tag: u8][expires_ms: i64][key_len: u32][val_len: u32][key_bytes][val_bytes]`.
///   The CRC32 (IEEE) covers every frame byte after the checksum field itself.
/// - Op tags: `1` = set, `2` = delete, `3` = batch begin, `4` = batch commit.
/// - A multi-operation [`KvStore::batch`] is written as one contiguous envelope:
///   `BEGIN(n)`, `n` set/delete records, `COMMIT(n)`. Batch markers have empty key and value
///   and carry the record count `n` in the `expires_ms` field. Logs written before batch
///   envelopes existed (tags `1`/`2` only) replay unchanged; older readers reject logs that
///   contain markers as an unknown operation tag.
///
/// # Batch Atomicity
/// Records inside an envelope are applied only once their `COMMIT` marker has been replayed.
/// An envelope cut off by a crash (torn record, missing commit, clean EOF mid-batch) is
/// dropped as a whole and truncated away together with the crash tail, so a batch is either
/// fully applied or not at all.
///
/// # Exclusive Access
/// [`FileStore::open`] creates `<path>.lock` (see [`lock_path`]) with `create_new` and holds it
/// until the store is dropped, so a second handle on the same WAL (in this or another
/// process) is rejected with an [`io::ErrorKind::AlreadyExists`] [`KvError::Io`] error
/// instead of silently losing writes after a compaction. If a process crashes, the stale lock
/// file stays behind (it records the owner PID for diagnosis); once you have verified that no
/// process still uses the store, call [`FileStore::break_lock`] to remove it.
///
/// # Compaction Failure
/// If compaction replaced the WAL but the store cannot reopen it afterwards, the store is
/// poisoned: reads keep serving the in-memory state, while every write, [`FileStore::sync`]
/// and [`FileStore::compact`] fails with an error. Drop and reopen the store to recover.
///
/// # Crash Recovery
/// On open, replay stops at the first frame that is incomplete, structurally invalid, or fails
/// its CRC. That frame is treated as a crash tail and truncated away when the remaining bytes
/// are all zero (preallocated / zero-filled tail) or when no CRC-valid frame starts anywhere
/// after it (torn final write). If a valid frame *does* follow, the log is corrupt in the
/// middle and opening fails with [`KvError::CorruptData`] instead of silently dropping data.
/// Frame lengths are bounded by the remaining file size and [`WAL_MAX_RECORD_LEN`] before any
/// allocation.
pub struct FileStore {
    path: PathBuf,
    clock: Arc<dyn Clock>,
    inner: Mutex<FileStoreInner>,
    _lock: LockFile,
}

impl fmt::Debug for FileStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let guard = self.lock();
        f.debug_struct("FileStore")
            .field("path", &self.path)
            .field("len", &guard.index.len())
            .field("dead_records", &guard.dead_records)
            .field("limits", &guard.limits)
            .finish()
    }
}

impl FileStoreInner {
    /// Fails if the store was poisoned by a failed post-compaction reopen.
    fn ensure_writable(&self) -> KvResult<()> {
        match &self.poisoned {
            Some(msg) => Err(KvError::Io(io::Error::other(msg.clone()))),
            None => Ok(()),
        }
    }

    /// Appends `buf` to the WAL. On failure the file is rolled back to the last complete
    /// record so a partial write never leaves garbage in the middle of the log.
    fn append(&mut self, buf: &[u8]) -> KvResult<()> {
        self.ensure_writable()?;
        if let Err(err) = self.file.write_all(buf).and_then(|()| self.file.flush()) {
            let _ = self.file.set_len(self.wal_len);
            let _ = self.file.seek(SeekFrom::Start(self.wal_len));
            return Err(KvError::Io(err));
        }
        self.wal_len += buf.len() as u64;
        Ok(())
    }
}

impl FileStore {
    /// Opens or creates a [`FileStore`] at `path` using [`SystemClock`] and default [`KvLimits`].
    ///
    /// Replays existing WAL records, verifies CRC32 checksums, truncates a torn or zero-filled
    /// crash tail (see [Crash Recovery](FileStore#crash-recovery)), and discards already-expired
    /// keys. Corruption followed by valid records returns [`KvError::CorruptData`].
    pub fn open(path: impl AsRef<Path>) -> KvResult<Self> {
        Self::open_with_options(
            path,
            Arc::new(SystemClock::new()),
            KvLimits::default(),
            None,
        )
    }

    /// Opens or creates a [`FileStore`] at `path` with an injected [`Clock`].
    pub fn open_with_clock<C: Clock + 'static>(path: impl AsRef<Path>, clock: C) -> KvResult<Self> {
        Self::open_with_options(path, Arc::new(clock), KvLimits::default(), None)
    }

    /// Opens or creates a [`FileStore`] at `path` with custom [`KvLimits`] and [`Clock`].
    pub fn open_with_limits<C: Clock + 'static>(
        path: impl AsRef<Path>,
        clock: C,
        limits: KvLimits,
    ) -> KvResult<Self> {
        Self::open_with_options(path, Arc::new(clock), limits, None)
    }

    fn open_with_options(
        path: impl AsRef<Path>,
        clock: Arc<dyn Clock>,
        limits: KvLimits,
        auto_compact_threshold: Option<usize>,
    ) -> KvResult<Self> {
        let path = path.as_ref().to_path_buf();
        validate_store_path(&path)?;
        // Taken first and released on every early return below (via Drop).
        let lock = LockFile::acquire(&path)?;

        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)?;

        let file_len = file.metadata()?.len();
        let mut index = BTreeMap::new();
        let mut dead_records = 0usize;

        if file_len == 0 {
            file.write_all(WAL_MAGIC)?;
            file.sync_all()?;
        } else {
            let valid_offset = Self::replay_wal(
                &mut file,
                file_len,
                &*clock,
                &limits,
                &mut index,
                &mut dead_records,
            )?;
            if valid_offset < file_len {
                // Truncate torn / zero-filled trailing bytes from an interrupted crash write.
                file.set_len(valid_offset)?;
                file.sync_all()?;
            }
            file.seek(SeekFrom::End(0))?;
        }
        let wal_len = file.metadata()?.len();

        Ok(Self {
            path,
            clock,
            inner: Mutex::new(FileStoreInner {
                file,
                wal_len,
                index,
                dead_records,
                auto_compact_threshold,
                limits,
                poisoned: None,
            }),
            _lock: lock,
        })
    }

    /// Removes the lock file of the WAL at `path`, returning `true` if one existed.
    ///
    /// Use this only to clear a stale lock left behind by a crashed process, after verifying
    /// that no live process (and no [`FileStore`] in this process) still has the store open;
    /// breaking a live lock allows two writers and can lose data.
    pub fn break_lock(path: impl AsRef<Path>) -> KvResult<bool> {
        match std::fs::remove_file(lock_path(path)) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(KvError::Io(err)),
        }
    }

    fn replay_wal(
        file: &mut File,
        file_len: u64,
        clock: &dyn Clock,
        limits: &KvLimits,
        index: &mut BTreeMap<String, FileEntry>,
        dead_records: &mut usize,
    ) -> KvResult<u64> {
        if file_len < WAL_MAGIC.len() as u64 {
            return Err(KvError::CorruptData(
                "WAL file shorter than 8-byte magic header".to_string(),
            ));
        }

        file.seek(SeekFrom::Start(0))?;
        let mut reader = BufReader::new(&mut *file);

        let mut magic = [0u8; 8];
        reader.read_exact(&mut magic)?;
        if &magic != WAL_MAGIC {
            return Err(KvError::CorruptData(
                "invalid WAL magic header (expected AKKUWAL\\x01)".to_string(),
            ));
        }

        let now_ms = datetime_to_unix_ms(clock.now());
        let mut offset = WAL_MAGIC.len() as u64;
        let mut header_buf = [0u8; FRAME_HEADER_LEN];
        let mut pending: Option<PendingBatch> = None;

        let invalid: Option<&'static str> = loop {
            let remaining = file_len - offset;
            if remaining == 0 {
                break None; // Clean EOF at a record boundary.
            }
            if remaining < FRAME_HEADER_LEN as u64 {
                break Some("truncated frame header");
            }
            reader.read_exact(&mut header_buf)?;

            let header = match parse_frame_header(&header_buf, remaining - FRAME_HEADER_LEN as u64)
            {
                Ok(h) => h,
                Err(reason) => break Some(reason),
            };

            // Both lengths are bounded by the remaining file size and WAL_MAX_RECORD_LEN.
            let mut key_bytes = vec![0u8; header.key_len];
            reader.read_exact(&mut key_bytes)?;
            let mut val_bytes = vec![0u8; header.val_len];
            reader.read_exact(&mut val_bytes)?;

            let actual_crc = compute_crc32(&[&header_buf[4..], &key_bytes, &val_bytes]);
            if actual_crc != header.crc {
                break Some("CRC32 mismatch");
            }

            if header.key_len > limits.max_key_size || header.val_len > limits.max_value_size {
                return Err(KvError::CorruptData(format!(
                    "WAL record at offset {offset} (key_len {}, val_len {}) exceeds configured \
                     KvLimits (max_key_size {}, max_value_size {}); reopen with larger limits",
                    header.key_len, header.val_len, limits.max_key_size, limits.max_value_size
                )));
            }

            let record_len = (FRAME_HEADER_LEN + header.key_len + header.val_len) as u64;
            match header.op_tag {
                OP_BATCH_BEGIN => {
                    if pending.is_some() {
                        return Err(KvError::CorruptData(format!(
                            "nested batch begin marker in WAL at offset {offset}"
                        )));
                    }
                    pending = Some(PendingBatch {
                        start: offset,
                        expected: header.expires_ms,
                        records: Vec::new(),
                    });
                }
                OP_BATCH_COMMIT => {
                    let batch = pending.take().ok_or_else(|| {
                        KvError::CorruptData(format!(
                            "batch commit marker without begin in WAL at offset {offset}"
                        ))
                    })?;
                    let count = batch.records.len() as i64;
                    if count != batch.expected || count != header.expires_ms {
                        return Err(KvError::CorruptData(format!(
                            "batch commit marker at offset {offset} closes {count} records but \
                             begin announced {} and commit {}",
                            batch.expected, header.expires_ms
                        )));
                    }
                    for (op_tag, key, entry) in batch.records {
                        apply_replayed(index, dead_records, op_tag, key, entry, now_ms);
                    }
                }
                op_tag => {
                    let key = String::from_utf8(key_bytes).map_err(|e| {
                        KvError::CorruptData(format!(
                            "non-UTF-8 key in WAL at offset {offset}: {e}"
                        ))
                    })?;
                    let entry = FileEntry {
                        value: val_bytes,
                        expires_ms: header.expires_ms,
                    };
                    match pending.as_mut() {
                        Some(batch) => {
                            if batch.records.len() as i64 >= batch.expected {
                                return Err(KvError::CorruptData(format!(
                                    "batch starting at offset {} holds more records than the \
                                     {} it announced",
                                    batch.start, batch.expected
                                )));
                            }
                            batch.records.push((op_tag, key, entry));
                        }
                        None => apply_replayed(index, dead_records, op_tag, key, entry, now_ms),
                    }
                }
            }

            offset += record_len;
        };
        drop(reader);

        if let Some(reason) = invalid {
            Self::check_recoverable_tail(file, file_len, offset, reason, limits)?;
        }
        // An envelope without its commit marker (crash mid-batch) is dropped as a whole.
        Ok(pending.map_or(offset, |batch| batch.start))
    }

    /// Decides whether the invalid record at `bad_offset` is a recoverable crash tail.
    ///
    /// The tail is recoverable (and the caller truncates it) when the remaining bytes are all
    /// zero (preallocated / zero-filled tail) or when no CRC-valid record starts anywhere after
    /// `bad_offset`. If a valid record follows, the log is corrupt in the middle and an error is
    /// returned instead of silently dropping data.
    fn check_recoverable_tail(
        file: &mut File,
        file_len: u64,
        bad_offset: u64,
        reason: &'static str,
        limits: &KvLimits,
    ) -> KvResult<()> {
        let tail_len = file_len - bad_offset;

        // 1. Zero-filled tail (streamed, so arbitrarily large preallocations are fine).
        file.seek(SeekFrom::Start(bad_offset))?;
        let mut all_zero = true;
        {
            let mut chunk = vec![0u8; 64 * 1024];
            let mut left = tail_len;
            while left > 0 {
                let want = left.min(chunk.len() as u64) as usize;
                file.read_exact(&mut chunk[..want])?;
                if chunk[..want].iter().any(|&b| b != 0) {
                    all_zero = false;
                    break;
                }
                left -= want as u64;
            }
        }
        if all_zero {
            return Ok(());
        }

        // 2. Non-zero garbage: a torn write affects at most the final record, so the tail can
        //    only legitimately be about one record long.
        let max_record = (FRAME_HEADER_LEN as u64)
            .saturating_add(limits.max_key_size as u64)
            .saturating_add(limits.max_value_size as u64)
            .min(FRAME_HEADER_LEN as u64 + WAL_MAX_RECORD_LEN);
        let scan_cap = max_record.saturating_mul(2).saturating_add(TAIL_SCAN_SLACK);
        if tail_len > scan_cap {
            return Err(KvError::CorruptData(format!(
                "WAL record at offset {bad_offset} is invalid ({reason}) and {tail_len} trailing \
                 bytes follow; refusing to truncate possibly valid data"
            )));
        }

        let mut tail = vec![0u8; tail_len as usize];
        file.seek(SeekFrom::Start(bad_offset))?;
        file.read_exact(&mut tail)?;

        match find_valid_frame(&tail[1..]) {
            Some(Ok(pos)) => Err(KvError::CorruptData(format!(
                "WAL record at offset {bad_offset} is corrupt ({reason}) but a valid record \
                 follows at offset {}; refusing to drop data",
                bad_offset + 1 + pos as u64
            ))),
            Some(Err(())) => Err(KvError::CorruptData(format!(
                "WAL record at offset {bad_offset} is invalid ({reason}) and the trailing bytes \
                 exceed the recovery scan budget; refusing to truncate"
            ))),
            None => Ok(()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FileStoreInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Configures the number of overwritten, deleted, or expired WAL records that triggers
    /// an automatic compaction on write.
    pub fn with_auto_compact_threshold(self, max_dead_records: usize) -> Self {
        self.lock().auto_compact_threshold = Some(max_dead_records);
        self
    }

    /// Configures custom [`KvLimits`] (`max_key_size` and `max_value_size`) on this store.
    pub fn with_limits(self, limits: KvLimits) -> Self {
        self.lock().limits = limits;
        self
    }

    /// Returns the configured [`KvLimits`].
    pub fn limits(&self) -> KvLimits {
        self.lock().limits
    }

    /// Atomically rewrites the WAL file containing only currently live, non-expired entries
    /// using [`akku::fs::write_atomic`](crate::fs::write_atomic).
    pub fn compact(&self) -> KvResult<()> {
        let mut guard = self.lock();
        self.compact_locked(&mut guard)
    }

    fn compact_locked(&self, guard: &mut FileStoreInner) -> KvResult<()> {
        guard.ensure_writable()?;
        let now_ms = datetime_to_unix_ms(self.clock.now());
        guard.index.retain(|_, entry| !entry.is_expired(now_ms));

        let mut snapshot = Vec::with_capacity(WAL_MAGIC.len() + guard.index.len() * 64);
        snapshot.extend_from_slice(WAL_MAGIC);

        for (key, entry) in &guard.index {
            encode_record(
                OP_SET,
                entry.expires_ms,
                key.as_bytes(),
                &entry.value,
                &mut snapshot,
            );
        }

        // Even when `write_atomic` fails, the rename may already have happened (e.g. the
        // directory fsync failed), so the old handle can be stale either way. Whichever file
        // now lives at `path` (old WAL or new snapshot) replays to the current index, so the
        // handle is always re-established from the path before reporting the outcome.
        let written = write_atomic(&self.path, &snapshot);

        let reopened = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.path)
            .and_then(|mut file| {
                let len = file.seek(SeekFrom::End(0))?;
                Ok((file, len))
            });
        match reopened {
            Ok((file, len)) => {
                guard.file = file;
                guard.wal_len = len;
            }
            Err(err) => {
                let msg = format!(
                    "FileStore '{}' is poisoned: the WAL could not be reopened after \
                     compaction ({err}); drop and reopen the store",
                    self.path.display()
                );
                guard.poisoned = Some(msg);
                return Err(KvError::Io(err));
            }
        }
        written?;
        guard.dead_records = 0;
        Ok(())
    }

    fn maybe_auto_compact(&self, guard: &mut FileStoreInner) -> KvResult<()> {
        if let Some(threshold) = guard.auto_compact_threshold {
            if guard.dead_records >= threshold {
                self.compact_locked(guard)?;
            }
        }
        Ok(())
    }

    /// Flushes and `sync_data`s the underlying WAL file handle.
    pub fn sync(&self) -> KvResult<()> {
        let mut guard = self.lock();
        guard.ensure_writable()?;
        guard.file.flush()?;
        guard.file.sync_data()?;
        Ok(())
    }

    /// Returns the filesystem path of the WAL file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the number of live (non-expired) keys in the store.
    pub fn len(&self) -> KvResult<usize> {
        let now_ms = datetime_to_unix_ms(self.clock.now());
        let guard = self.lock();
        Ok(guard
            .index
            .values()
            .filter(|e| !e.is_expired(now_ms))
            .count())
    }

    /// Returns `true` if the store has no live keys.
    pub fn is_empty(&self) -> KvResult<bool> {
        Ok(self.len()? == 0)
    }

    /// Returns the current on-disk size of the WAL file in bytes.
    pub fn disk_size(&self) -> KvResult<u64> {
        let guard = self.lock();
        Ok(guard.file.metadata()?.len())
    }
}

impl KvStore for FileStore {
    fn get(&self, key: &str) -> KvResult<Option<Vec<u8>>> {
        let now_ms = datetime_to_unix_ms(self.clock.now());
        let mut guard = self.lock();
        guard.limits.validate_key(key)?;
        if let Some(entry) = guard.index.get(key) {
            if !entry.is_expired(now_ms) {
                return Ok(Some(entry.value.clone()));
            }
            guard.index.remove(key);
            guard.dead_records += 1;
        }
        Ok(None)
    }

    fn set(&self, key: &str, value: &[u8]) -> KvResult<()> {
        let mut guard = self.lock();
        guard.limits.validate_kv(key, value)?;

        let mut buf = Vec::new();
        encode_record(OP_SET, NO_EXPIRY, key.as_bytes(), value, &mut buf);
        guard.append(&buf)?;

        if guard
            .index
            .insert(
                key.to_string(),
                FileEntry {
                    value: value.to_vec(),
                    expires_ms: NO_EXPIRY,
                },
            )
            .is_some()
        {
            guard.dead_records += 1;
        }
        self.maybe_auto_compact(&mut guard)
    }

    fn set_ex(&self, key: &str, value: &[u8], ttl: Duration) -> KvResult<()> {
        let expires_ms = compute_expires_ms(&*self.clock, ttl);
        let now_ms = datetime_to_unix_ms(self.clock.now());
        let mut guard = self.lock();
        guard.limits.validate_kv(key, value)?;

        let mut buf = Vec::new();
        encode_record(OP_SET, expires_ms, key.as_bytes(), value, &mut buf);
        guard.append(&buf)?;

        let entry = FileEntry {
            value: value.to_vec(),
            expires_ms,
        };
        if entry.is_expired(now_ms) {
            if guard.index.remove(key).is_some() {
                guard.dead_records += 1;
            }
            guard.dead_records += 1;
        } else if guard.index.insert(key.to_string(), entry).is_some() {
            guard.dead_records += 1;
        }
        self.maybe_auto_compact(&mut guard)
    }

    fn delete(&self, key: &str) -> KvResult<bool> {
        let now_ms = datetime_to_unix_ms(self.clock.now());
        let mut guard = self.lock();
        guard.limits.validate_key(key)?;

        let mut buf = Vec::new();
        encode_record(OP_DELETE, NO_EXPIRY, key.as_bytes(), &[], &mut buf);
        guard.append(&buf)?;

        let existed_live = match guard.index.remove(key) {
            Some(old) => !old.is_expired(now_ms),
            None => false,
        };

        guard.dead_records += if existed_live { 2 } else { 1 };
        self.maybe_auto_compact(&mut guard)?;
        Ok(existed_live)
    }

    fn exists(&self, key: &str) -> KvResult<bool> {
        let now_ms = datetime_to_unix_ms(self.clock.now());
        let guard = self.lock();
        guard.limits.validate_key(key)?;
        Ok(guard.index.get(key).is_some_and(|e| !e.is_expired(now_ms)))
    }

    fn scan_prefix(&self, prefix: &str) -> KvResult<Vec<(String, Vec<u8>)>> {
        let now_ms = datetime_to_unix_ms(self.clock.now());
        let guard = self.lock();
        let mut results = Vec::new();
        for (k, entry) in guard.index.range(prefix.to_string()..) {
            if !k.starts_with(prefix) {
                break;
            }
            if !entry.is_expired(now_ms) {
                results.push((k.clone(), entry.value.clone()));
            }
        }
        Ok(results)
    }

    fn scan_range(&self, start: &str, end: &str) -> KvResult<Vec<(String, Vec<u8>)>> {
        if start >= end {
            return Ok(Vec::new());
        }
        let now_ms = datetime_to_unix_ms(self.clock.now());
        let guard = self.lock();
        let mut results = Vec::new();
        for (k, entry) in guard.index.range(start.to_string()..end.to_string()) {
            if !entry.is_expired(now_ms) {
                results.push((k.clone(), entry.value.clone()));
            }
        }
        Ok(results)
    }

    fn batch(&self, ops: &[BatchOp]) -> KvResult<()> {
        let now_ms = datetime_to_unix_ms(self.clock.now());
        let mut guard = self.lock();

        // Pre-validate all operations and serialize into a single contiguous WAL buffer.
        // Multi-op batches are wrapped in a BEGIN/COMMIT envelope so a torn write can never
        // replay a prefix of the batch.
        let mut buf = Vec::new();
        let mut computed_expiries = Vec::with_capacity(ops.len());
        let envelope = ops.len() > 1;
        let count = i64::try_from(ops.len()).unwrap_or(i64::MAX);
        if envelope {
            encode_record(OP_BATCH_BEGIN, count, &[], &[], &mut buf);
        }

        for op in ops {
            match op {
                BatchOp::Set { key, value } => {
                    guard.limits.validate_kv(key, value)?;
                    encode_record(OP_SET, NO_EXPIRY, key.as_bytes(), value, &mut buf);
                    computed_expiries.push(NO_EXPIRY);
                }
                BatchOp::SetEx { key, value, ttl } => {
                    guard.limits.validate_kv(key, value)?;
                    let exp = compute_expires_ms(&*self.clock, *ttl);
                    encode_record(OP_SET, exp, key.as_bytes(), value, &mut buf);
                    computed_expiries.push(exp);
                }
                BatchOp::Delete { key } => {
                    guard.limits.validate_key(key)?;
                    encode_record(OP_DELETE, NO_EXPIRY, key.as_bytes(), &[], &mut buf);
                    computed_expiries.push(NO_EXPIRY);
                }
            }
        }

        if envelope {
            encode_record(OP_BATCH_COMMIT, count, &[], &[], &mut buf);
        }
        if !buf.is_empty() {
            guard.append(&buf)?;
        }

        for (op, expires_ms) in ops.iter().zip(computed_expiries) {
            match op {
                BatchOp::Set { key, value } | BatchOp::SetEx { key, value, .. } => {
                    let entry = FileEntry {
                        value: value.clone(),
                        expires_ms,
                    };
                    if entry.is_expired(now_ms) {
                        if guard.index.remove(key).is_some() {
                            guard.dead_records += 1;
                        }
                        guard.dead_records += 1;
                    } else if guard.index.insert(key.clone(), entry).is_some() {
                        guard.dead_records += 1;
                    }
                }
                BatchOp::Delete { key } => {
                    if guard.index.remove(key).is_some() {
                        guard.dead_records += 2;
                    } else {
                        guard.dead_records += 1;
                    }
                }
            }
        }

        self.maybe_auto_compact(&mut guard)
    }
}
