//! Raw DEFLATE compression and decompression (RFC 1951).
//!
//! Pure safe Rust on `std` only. Decompression always runs with a mandatory
//! `max_output_bytes` cap (zip-bomb guard): the decoder's memory use is
//! bounded by the cap plus a fixed window, no matter how small the compressed
//! input is.
//!
//! The encoder is deterministic: its output depends only on the input bytes
//! and the chosen [`CompressionLevel`] — never on how the input was chunked
//! into writes. This is a hard guarantee, so a streaming [`DeflateEncoder`]
//! and the one-shot [`deflate`] produce byte-identical output for the same
//! input.
//!
//! # Examples
//!
//! ```
//! use akku::archive::deflate::{self, CompressionLevel};
//!
//! let data = b"hello deflate, hello deflate, hello!".as_slice();
//! let packed = deflate::deflate(data, CompressionLevel::Best);
//! let unpacked = deflate::inflate(&packed, 1 << 20).unwrap();
//! assert_eq!(unpacked, data);
//! ```

use std::cmp::min;
use std::collections::BinaryHeap;
use std::fmt;
use std::io::{self, Read, Write};
use std::mem;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Compression tiers, ordered by increasing effort and ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum CompressionLevel {
    /// Stored blocks only: valid DEFLATE, no compression.
    Stored,
    /// Greedy LZ77 with short hash chains and fixed Huffman codes.
    Fast,
    /// Lazy-matching LZ77 with fixed Huffman codes. The default.
    #[default]
    Normal,
    /// Lazy-matching LZ77 with per-block dynamic Huffman codes.
    Best,
}

impl CompressionLevel {
    /// True when this level emits stored blocks only.
    pub fn is_stored(self) -> bool {
        self == Self::Stored
    }
}

/// Errors returned when decoding (or streaming) DEFLATE data.
#[derive(Debug)]
#[non_exhaustive]
pub enum DeflateError {
    /// The compressed stream ended before the final block completed.
    TruncatedStream,
    /// A block header used the reserved type 3 (`BTYPE == 11`).
    InvalidBlockType(u8),
    /// A stored block's `LEN` and one's-complement `NLEN` disagreed.
    InvalidStoredBlockLen,
    /// A Huffman table or symbol was malformed: over-subscribed table,
    /// invalid code, or an invalid repeat in the code-length encoding.
    InvalidHuffmanCode,
    /// A match referenced a distance beyond the 32 KiB window, or before the
    /// start of the output produced so far.
    DistanceTooFarBack,
    /// The zip-bomb guard tripped: decoded output would exceed the cap.
    OutputLimitExceeded,
    /// An I/O error on the underlying reader or writer.
    Io(io::Error),
}

impl fmt::Display for DeflateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedStream => f.write_str("compressed stream ended before the final block"),
            Self::InvalidBlockType(t) => write!(f, "invalid block type {t}"),
            Self::InvalidStoredBlockLen => f.write_str("stored block LEN/NLEN mismatch"),
            Self::InvalidHuffmanCode => f.write_str("invalid Huffman code or table"),
            Self::DistanceTooFarBack => write!(f, "match distance exceeds the window or output"),
            Self::OutputLimitExceeded => {
                write!(f, "decompressed output exceeds the configured cap")
            }
            Self::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for DeflateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for DeflateError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

// ---------------------------------------------------------------------------
// One-shot API
// ---------------------------------------------------------------------------

/// Compress `data` as a raw RFC 1951 DEFLATE stream. Never fails.
///
/// ```
/// use akku::archive::deflate::{self, CompressionLevel};
///
/// // The empty stream at a Huffman level is exactly the fixed-code final
/// // block: BFINAL=1, BTYPE=01, end-of-block code 0000000.
/// assert_eq!(deflate::deflate(b"", CompressionLevel::Normal), [0x03, 0x00]);
/// assert_eq!(
///     deflate::deflate(b"", CompressionLevel::Stored),
///     [0x01, 0x00, 0x00, 0xFF, 0xFF]
/// );
/// ```
pub fn deflate(data: &[u8], level: CompressionLevel) -> Vec<u8> {
    let mut core = EncoderCore::new(level);
    core.push(data);
    core.finish();
    mem::take(&mut core.sink.out)
}

/// Decompress a raw RFC 1951 DEFLATE stream with a hard output cap.
///
/// `max_output_bytes` bounds the total decoded size (zip-bomb guard); the
/// decoder's memory use stays at cap + window regardless of the input.
/// Bytes after the final block are ignored.
///
/// ```
/// use akku::archive::deflate::{self, CompressionLevel};
///
/// let data = b"tiny".as_slice();
/// let packed = deflate::deflate(data, CompressionLevel::Normal);
/// assert_eq!(deflate::inflate(&packed, 1024).unwrap(), data);
/// ```
pub fn inflate(data: &[u8], max_output_bytes: u64) -> Result<Vec<u8>, DeflateError> {
    let mut stream = InflateStream::new(max_output_bytes);
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut scratch = vec![0u8; 8192];
    while !stream.is_finished() {
        let mut progressed = false;
        if pos < data.len() {
            let before = stream.total_out;
            let consumed = stream.push(&data[pos..])?;
            pos += consumed;
            progressed |= consumed > 0 || stream.total_out > before || stream.is_done();
        }
        loop {
            let n = stream.pull(&mut scratch);
            if n == 0 {
                break;
            }
            out.extend_from_slice(&scratch[..n]);
            progressed = true;
        }
        if stream.is_finished() {
            break;
        }
        if !progressed {
            // Input exhausted (or stalled) without completing the stream.
            return Err(DeflateError::TruncatedStream);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Streaming decoder core
// ---------------------------------------------------------------------------

/// Sliding-window size (RFC 1951 maximum match distance).
const WINDOW_SIZE: usize = 32_768;

/// Size of the decode ring buffer; `push` pauses while the ring is full and
/// the caller has not drained it via [`InflateStream::pull`].
const RING_SIZE: usize = 65_536;

const RING_MASK: u64 = (RING_SIZE - 1) as u64;

/// Decode state machine. `final` flags ride along inside `Stored`/`Block`
/// so a stream may pause anywhere and resume exactly where it stopped.
#[derive(Debug)]
enum StreamState {
    /// Waiting for the next block header.
    WantHeader,
    /// Inside a stored block with `remaining` bytes left.
    Stored { remaining: u32, final_block: bool },
    /// Inside a Huffman block.
    Block {
        lit: HuffTable,
        dist: HuffTable,
        final_block: bool,
    },
    /// The final block ended; only draining is left.
    Done,
}

/// Incremental RFC 1951 decoder with push/pull boundaries.
///
/// Feed compressed bytes with [`push`](InflateStream::push), drain
/// decompressed bytes with [`pull`](InflateStream::pull). While a block is
/// active, `pull` retains the 32 KiB match window; once the stream is
/// finished, everything drains.
///
/// # Examples
///
/// ```
/// use akku::archive::deflate::{self, CompressionLevel, InflateStream};
///
/// let packed = deflate::deflate(b"stream me, stream me", CompressionLevel::Normal);
/// let mut stream = InflateStream::new(1 << 16);
/// stream.push(&packed).unwrap();
/// let mut out = Vec::new();
/// let mut buf = [0u8; 4096];
/// loop {
///     let n = stream.pull(&mut buf);
///     if n == 0 {
///         break;
///     }
///     out.extend_from_slice(&buf[..n]);
/// }
/// assert!(stream.is_finished());
/// assert_eq!(out, b"stream me, stream me");
/// ```
#[derive(Debug)]
pub struct InflateStream {
    cap: u64,
    total_out: u64,
    drained: u64,
    ring: Vec<u8>,
    bitbuf: u64,
    bitcnt: u32,
    state: StreamState,
}

impl InflateStream {
    /// A fresh decoder bounded by `max_output_bytes` total output.
    pub fn new(max_output_bytes: u64) -> Self {
        Self {
            cap: max_output_bytes,
            total_out: 0,
            drained: 0,
            ring: vec![0u8; RING_SIZE],
            bitbuf: 0,
            bitcnt: 0,
            state: StreamState::WantHeader,
        }
    }

    /// True once the final block ended and all output has been drained.
    pub fn is_finished(&self) -> bool {
        matches!(self.state, StreamState::Done) && self.drained == self.total_out
    }

    /// Feed compressed input; returns how many bytes were consumed.
    ///
    /// Bytes after the final block are left unconsumed (the gzip layer reads
    /// its trailer from them). If the caller provides more input than the
    /// decoder can currently process (output ring full), the remainder is
    /// left for a later call.
    ///
    /// When the input ends in the middle of a symbol or block header, the
    /// incomplete tail is **not** consumed (the return value may even be 0):
    /// the caller must keep those bytes and pass them again, followed by more
    /// input, on the next call. After an error the stream must not be reused.
    pub fn push(&mut self, input: &[u8]) -> Result<usize, DeflateError> {
        if matches!(self.state, StreamState::Done) {
            return Ok(0);
        }
        let mut src = BitSource {
            input,
            pos: 0,
            bitbuf: self.bitbuf,
            bitcnt: self.bitcnt,
        };
        let result = self.run(&mut src);
        // Whole bytes still sitting in the bit accumulator are handed back
        // to the caller as unconsumed input (they are the most recently read
        // bytes of `input`); only the partial byte's leftover bits are kept.
        // Keeping them *and* reporting them unconsumed would duplicate them
        // on the next call.
        let whole = (src.bitcnt / 8) as usize;
        let keep = src.bitcnt % 8;
        self.bitcnt = keep;
        self.bitbuf = src.bitbuf & ((1u64 << keep) - 1);
        let consumed = src.pos - whole;
        result.map(|()| consumed)
    }

    /// Drain decompressed bytes into `out`; returns the number written.
    pub fn pull(&mut self, out: &mut [u8]) -> usize {
        let keep = if matches!(self.state, StreamState::Done) {
            0
        } else {
            WINDOW_SIZE as u64
        };
        let drainable = self.total_out.saturating_sub(self.drained + keep);
        let n = min(out.len() as u64, drainable) as usize;
        let mut copied = 0usize;
        while copied < n {
            let from = (self.drained & RING_MASK) as usize;
            let contiguous = min(n - copied, RING_SIZE - from);
            out[copied..copied + contiguous].copy_from_slice(&self.ring[from..from + contiguous]);
            self.drained += contiguous as u64;
            copied += contiguous;
        }
        n
    }

    /// Total bytes decoded so far (drained or not).
    pub(crate) fn total_out(&self) -> u64 {
        self.total_out
    }

    /// True once the final block ended (output may still be pending).
    pub(crate) fn is_done(&self) -> bool {
        matches!(self.state, StreamState::Done)
    }

    fn ring_space(&self) -> u64 {
        RING_SIZE as u64 - (self.total_out - self.drained)
    }

    /// Run the state machine as far as the input and ring allow.
    fn run(&mut self, src: &mut BitSource<'_>) -> Result<(), DeflateError> {
        loop {
            match mem::replace(&mut self.state, StreamState::WantHeader) {
                StreamState::WantHeader => match self.read_header(src)? {
                    Some(next) => self.state = next,
                    None => return Ok(()),
                },
                StreamState::Stored {
                    remaining,
                    final_block,
                } => {
                    self.run_stored(src, remaining, final_block)?;
                    if matches!(self.state, StreamState::WantHeader) {
                        // still running: run_stored only pauses into Stored
                        continue;
                    }
                    // Done is terminal for this call.
                    return Ok(());
                }
                StreamState::Block {
                    lit,
                    dist,
                    final_block,
                } => {
                    let finished = self.run_huff(src, lit, dist, final_block)?;
                    if !finished {
                        return Ok(());
                    }
                }
                StreamState::Done => {
                    self.state = StreamState::Done;
                    return Ok(());
                }
            }
        }
    }

    fn read_header(
        &mut self,
        src: &mut BitSource<'_>,
    ) -> Result<Option<StreamState>, DeflateError> {
        let snap = (src.bitbuf, src.bitcnt, src.pos);
        let final_flag = match src.bit() {
            Some(b) => b,
            None => return Ok(self.restore(src, snap)),
        };
        let btype = match src.bits(2) {
            Some(b) => b,
            None => return Ok(self.restore(src, snap)),
        };
        match btype {
            0 => {
                // Stored blocks skip to the next byte boundary.
                let pad = src.bitcnt % 8;
                if src.bits(pad).is_none() {
                    return Ok(self.restore(src, snap));
                }
                let len = match src.bits(16) {
                    Some(v) => v as u16,
                    None => return Ok(self.restore(src, snap)),
                };
                let nlen = match src.bits(16) {
                    Some(v) => v as u16,
                    None => return Ok(self.restore(src, snap)),
                };
                if len != !nlen {
                    return Err(DeflateError::InvalidStoredBlockLen);
                }
                Ok(Some(StreamState::Stored {
                    remaining: len as u32,
                    final_block: final_flag == 1,
                }))
            }
            1 => {
                let lit = HuffTable::new(&FIXED_LIT_LENGTHS)?;
                let dist = HuffTable::new(&FIXED_DIST_LENGTHS)?;
                Ok(Some(StreamState::Block {
                    lit,
                    dist,
                    final_block: final_flag == 1,
                }))
            }
            2 => {
                let hlit = match src.bits(5) {
                    Some(v) => v as usize + 257,
                    None => return Ok(self.restore(src, snap)),
                };
                let hdist = match src.bits(5) {
                    Some(v) => v as usize + 1,
                    None => return Ok(self.restore(src, snap)),
                };
                let hclen = match src.bits(4) {
                    Some(v) => v as usize + 4,
                    None => return Ok(self.restore(src, snap)),
                };
                if hlit > 286 || hdist > 30 {
                    return Err(DeflateError::InvalidHuffmanCode);
                }
                let mut cl_lengths = [0u8; 19];
                for k in 0..hclen {
                    match src.bits(3) {
                        Some(v) => cl_lengths[ORDER[k]] = v as u8,
                        None => return Ok(self.restore(src, snap)),
                    }
                }
                let cl_table = HuffTable::new(&cl_lengths)?;
                let mut lengths = vec![0u8; hlit + hdist];
                let mut i = 0usize;
                let mut prev: i16 = -1;
                while i < lengths.len() {
                    let sym = match cl_table.decode(src) {
                        Ok(Some(s)) => s,
                        Ok(None) => return Ok(self.restore(src, snap)),
                        Err(e) => return Err(e),
                    };
                    match sym {
                        0..=15 => {
                            prev = sym as i16;
                            lengths[i] = sym as u8;
                            i += 1;
                        }
                        16 => {
                            if prev < 0 {
                                return Err(DeflateError::InvalidHuffmanCode);
                            }
                            let rep = match src.bits(2) {
                                Some(v) => 3 + v as usize,
                                None => return Ok(self.restore(src, snap)),
                            };
                            if i + rep > lengths.len() {
                                return Err(DeflateError::InvalidHuffmanCode);
                            }
                            for _ in 0..rep {
                                lengths[i] = prev as u8;
                                i += 1;
                            }
                        }
                        17 => {
                            let rep = match src.bits(3) {
                                Some(v) => 3 + v as usize,
                                None => return Ok(self.restore(src, snap)),
                            };
                            if i + rep > lengths.len() {
                                return Err(DeflateError::InvalidHuffmanCode);
                            }
                            prev = 0;
                            for _ in 0..rep {
                                lengths[i] = 0;
                                i += 1;
                            }
                        }
                        18 => {
                            let rep = match src.bits(7) {
                                Some(v) => 11 + v as usize,
                                None => return Ok(self.restore(src, snap)),
                            };
                            if i + rep > lengths.len() {
                                return Err(DeflateError::InvalidHuffmanCode);
                            }
                            prev = 0;
                            for _ in 0..rep {
                                lengths[i] = 0;
                                i += 1;
                            }
                        }
                        _ => return Err(DeflateError::InvalidHuffmanCode),
                    }
                }
                let lit = HuffTable::new(&lengths[..hlit])?;
                let dist = HuffTable::new(&lengths[hlit..])?;
                Ok(Some(StreamState::Block {
                    lit,
                    dist,
                    final_block: final_flag == 1,
                }))
            }
            other => Err(DeflateError::InvalidBlockType(other as u8)),
        }
    }

    /// Restore a snapshot and report "paused" (`None`).
    fn restore(&mut self, src: &mut BitSource<'_>, snap: (u64, u32, usize)) -> Option<StreamState> {
        src.bitbuf = snap.0;
        src.bitcnt = snap.1;
        src.pos = snap.2;
        None
    }

    /// Copy stored-block bytes; sets `self.state` and returns false on pause.
    fn run_stored(
        &mut self,
        src: &mut BitSource<'_>,
        mut remaining: u32,
        final_block: bool,
    ) -> Result<bool, DeflateError> {
        while remaining > 0 {
            if self.ring_space() == 0 {
                self.state = StreamState::Stored {
                    remaining,
                    final_block,
                };
                return Ok(false);
            }
            if self.total_out >= self.cap {
                return Err(DeflateError::OutputLimitExceeded);
            }
            let snap = (src.bitbuf, src.bitcnt, src.pos);
            match src.bits(8) {
                Some(b) => {
                    self.ring[(self.total_out & RING_MASK) as usize] = b as u8;
                    self.total_out += 1;
                    remaining -= 1;
                }
                None => {
                    src.bitbuf = snap.0;
                    src.bitcnt = snap.1;
                    src.pos = snap.2;
                    self.state = StreamState::Stored {
                        remaining,
                        final_block,
                    };
                    return Ok(false);
                }
            }
        }
        self.state = if final_block {
            StreamState::Done
        } else {
            StreamState::WantHeader
        };
        Ok(true)
    }

    /// Decode Huffman symbols; sets `self.state` and returns false on pause.
    fn run_huff(
        &mut self,
        src: &mut BitSource<'_>,
        lit: HuffTable,
        dist: HuffTable,
        final_block: bool,
    ) -> Result<bool, DeflateError> {
        loop {
            if self.ring_space() == 0 {
                self.state = StreamState::Block {
                    lit,
                    dist,
                    final_block,
                };
                return Ok(false);
            }
            let snap = (src.bitbuf, src.bitcnt, src.pos);
            let pause =
                |me: &mut Self, src: &mut BitSource<'_>, lit: HuffTable, dist: HuffTable| {
                    src.bitbuf = snap.0;
                    src.bitcnt = snap.1;
                    src.pos = snap.2;
                    me.state = StreamState::Block {
                        lit,
                        dist,
                        final_block,
                    };
                };
            let sym = match lit.decode(src) {
                Ok(Some(s)) => s,
                Ok(None) => {
                    pause(self, src, lit, dist);
                    return Ok(false);
                }
                Err(e) => return Err(e),
            };
            if sym < 256 {
                if self.total_out >= self.cap {
                    return Err(DeflateError::OutputLimitExceeded);
                }
                self.ring[(self.total_out & RING_MASK) as usize] = sym as u8;
                self.total_out += 1;
            } else if sym == 256 {
                self.state = if final_block {
                    StreamState::Done
                } else {
                    StreamState::WantHeader
                };
                return Ok(true);
            } else {
                if sym > 285 {
                    return Err(DeflateError::InvalidHuffmanCode);
                }
                let idx = (sym - 257) as usize;
                let extra = LEN_EXTRA[idx];
                let len = match src.bits(extra) {
                    Some(v) => LEN_BASE[idx] + v,
                    None => {
                        pause(self, src, lit, dist);
                        return Ok(false);
                    }
                };
                let dsym = match dist.decode(src) {
                    Ok(Some(s)) => s,
                    Ok(None) => {
                        pause(self, src, lit, dist);
                        return Ok(false);
                    }
                    Err(e) => return Err(e),
                };
                if dsym > 29 {
                    return Err(DeflateError::InvalidHuffmanCode);
                }
                let dextra = DIST_EXTRA[dsym as usize];
                let dist_val = match src.bits(dextra) {
                    Some(v) => DIST_BASE[dsym as usize] + v,
                    None => {
                        pause(self, src, lit, dist);
                        return Ok(false);
                    }
                };
                if dist_val as u64 > self.total_out || dist_val as usize > WINDOW_SIZE {
                    return Err(DeflateError::DistanceTooFarBack);
                }
                if self.total_out + len as u64 > self.cap {
                    return Err(DeflateError::OutputLimitExceeded);
                }
                if self.ring_space() < len as u64 {
                    pause(self, src, lit, dist);
                    return Ok(false);
                }
                let base = self.total_out;
                let src_base = base - dist_val as u64;
                for i in 0..len as u64 {
                    let b = self.ring[((src_base + i) & RING_MASK) as usize];
                    self.ring[((base + i) & RING_MASK) as usize] = b;
                }
                self.total_out += len as u64;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Streaming decoder wrapper over any reader
// ---------------------------------------------------------------------------

/// Streaming raw-DEFLATE decoder over any [`Read`].
///
/// ```
/// use akku::archive::deflate::{self, CompressionLevel, InflateDecoder};
/// use std::io::Read;
///
/// let packed = deflate::deflate(b"read me", CompressionLevel::Normal);
/// let mut decoder = InflateDecoder::new(&packed[..], 1024);
/// let mut out = Vec::new();
/// decoder.read_to_end(&mut out).unwrap();
/// decoder.finish().unwrap();
/// assert_eq!(out, b"read me");
/// ```
#[derive(Debug)]
pub struct InflateDecoder<R: Read> {
    input: R,
    stream: InflateStream,
    buf: Vec<u8>,
    bpos: usize,
    blen: usize,
    eof: bool,
}

impl<R: Read> InflateDecoder<R> {
    /// A decoder reading compressed bytes from `input`, bounded by
    /// `max_output_bytes` total output.
    pub fn new(input: R, max_output_bytes: u64) -> Self {
        Self {
            input,
            stream: InflateStream::new(max_output_bytes),
            buf: vec![0u8; 64 * 1024],
            bpos: 0,
            blen: 0,
            eof: false,
        }
    }

    /// Consume and verify: the stream must have ended cleanly.
    pub fn finish(self) -> Result<(), DeflateError> {
        if self.stream.is_finished() {
            Ok(())
        } else {
            Err(DeflateError::TruncatedStream)
        }
    }

    /// Compact the unconsumed tail to the front of the buffer and append
    /// more input after it. Returns `false` at end of input.
    fn refill(&mut self) -> io::Result<bool> {
        if self.eof {
            return Ok(false);
        }
        if self.bpos > 0 {
            self.buf.copy_within(self.bpos..self.blen, 0);
            self.blen -= self.bpos;
            self.bpos = 0;
        }
        if self.blen == self.buf.len() {
            // A full 64 KiB buffer without progress cannot be a valid stream
            // (no header or symbol is that long).
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                DeflateError::InvalidHuffmanCode,
            ));
        }
        loop {
            match self.input.read(&mut self.buf[self.blen..]) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(false);
                }
                Ok(n) => {
                    self.blen += n;
                    return Ok(true);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

impl<R: Read> Read for InflateDecoder<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        // Without room to drain, a finished stream with trailing input would
        // spin forever and a long stream would fill the ring and the input
        // buffer, so an empty read is a no-op.
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            // With a non-empty `out`, every iteration either drains output,
            // consumes input, produces output, or reads more input: once the
            // stream is done, `pull` drains everything left (no window is
            // kept), so a zero `pull` then means `is_finished`.
            let n = self.stream.pull(out);
            if n > 0 {
                return Ok(n);
            }
            if self.stream.is_finished() {
                return Ok(0);
            }
            if self.bpos < self.blen {
                let before = self.stream.total_out;
                let consumed = self
                    .stream
                    .push(&self.buf[self.bpos..self.blen])
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                self.bpos += consumed;
                if consumed > 0 || self.stream.total_out > before || self.stream.is_done() {
                    continue;
                }
            }
            // No buffered input, or only an incomplete symbol: read more.
            if !self.refill()? {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    DeflateError::TruncatedStream,
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Streaming encoder
// ---------------------------------------------------------------------------

/// Streaming raw-DEFLATE encoder over any [`Write`].
///
/// Output depends only on the input bytes, never on the write chunking.
/// Call [`finish`](DeflateEncoder::finish) to emit the final block and flush;
/// dropping without finishing performs a best-effort flush (trailing errors
/// are discarded).
///
/// ```
/// use akku::archive::deflate::{self, CompressionLevel, DeflateEncoder};
/// use std::io::Write;
///
/// let mut enc = DeflateEncoder::new(Vec::new(), CompressionLevel::Normal);
/// enc.write_all(b"chunk one ").unwrap();
/// enc.write_all(b"chunk").unwrap();
/// enc.write_all(b" two").unwrap();
/// let packed = enc.finish().unwrap();
/// assert_eq!(deflate::inflate(&packed, 1024).unwrap(), b"chunk one chunk two");
/// ```
#[derive(Debug)]
pub struct DeflateEncoder<W: Write> {
    core: EncoderCore,
    out: Option<W>,
    finished: bool,
}

impl<W: Write> DeflateEncoder<W> {
    /// An encoder writing compressed bytes to `out` at `level`.
    pub fn new(out: W, level: CompressionLevel) -> Self {
        Self {
            core: EncoderCore::new(level),
            out: Some(out),
            finished: false,
        }
    }

    fn drain(&mut self) -> io::Result<()> {
        if self.core.sink.out.is_empty() {
            return Ok(());
        }
        let bytes = mem::take(&mut self.core.sink.out);
        match self.out.as_mut() {
            Some(w) => w.write_all(&bytes),
            None => Err(io::Error::other("deflate encoder finished")),
        }
    }

    /// Emit the final block, flush, and return the inner writer.
    pub fn finish(mut self) -> io::Result<W> {
        self.core.finish();
        self.drain()?;
        self.finished = true;
        match self.out.take() {
            Some(mut w) => {
                w.flush()?;
                Ok(w)
            }
            None => Err(io::Error::other("deflate encoder already finished")),
        }
    }
}

impl<W: Write> Write for DeflateEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.out.is_none() {
            return Err(io::Error::other("deflate encoder finished"));
        }
        self.core.push(buf);
        self.drain()?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.drain()?;
        match self.out.as_mut() {
            Some(w) => w.flush(),
            None => Ok(()),
        }
    }
}

impl<W: Write> Drop for DeflateEncoder<W> {
    fn drop(&mut self) {
        if !self.finished && self.out.is_some() {
            self.core.finish();
            let _ = self.drain();
            if let Some(w) = self.out.as_mut() {
                let _ = w.flush();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Bit sink/source and Huffman primitives
// ---------------------------------------------------------------------------

/// LSB-first bit writer.
#[derive(Debug)]
struct BitSink {
    out: Vec<u8>,
    buf: u64,
    cnt: u32,
}

impl BitSink {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            buf: 0,
            cnt: 0,
        }
    }

    /// Write the low `n` bits of `value`, least-significant bit first.
    fn put(&mut self, value: u32, n: u32) {
        if n == 0 {
            return;
        }
        self.buf |= (value as u64) << self.cnt;
        self.cnt += n;
        while self.cnt >= 8 {
            self.out.push((self.buf & 0xFF) as u8);
            self.buf >>= 8;
            self.cnt -= 8;
        }
    }

    /// Write a Huffman code given in natural (MSB-first) order.
    fn put_code(&mut self, code: u16, len: u8) {
        self.put(reverse_bits(code as u32, len as u32), len as u32);
    }

    /// Pad to the next byte boundary with zero bits.
    fn align(&mut self) {
        if self.cnt > 0 {
            self.out.push((self.buf & 0xFF) as u8);
            self.buf = 0;
            self.cnt = 0;
        }
    }
}

fn reverse_bits(code: u32, len: u32) -> u32 {
    let mut r = 0u32;
    for i in 0..len {
        r |= ((code >> i) & 1) << (len - 1 - i);
    }
    r
}

/// Canonical Huffman codes from code lengths (RFC 1951 §3.2.2).
fn canonical_codes(lengths: &[u8]) -> Vec<u16> {
    let max = lengths.iter().copied().max().unwrap_or(0) as usize;
    let mut bl_count = vec![0u16; max + 1];
    for &l in lengths {
        if l > 0 {
            bl_count[l as usize] += 1;
        }
    }
    let mut next_code = vec![0u32; max + 2];
    let mut code = 0u32;
    for bits in 1..=max {
        code = (code + bl_count[bits - 1] as u32) << 1;
        next_code[bits] = code;
    }
    let mut codes = vec![0u16; lengths.len()];
    for (sym, &l) in lengths.iter().enumerate() {
        if l > 0 {
            codes[sym] = next_code[l as usize] as u16;
            next_code[l as usize] += 1;
        }
    }
    codes
}

/// Huffman code lengths for the given frequencies, limited to `limit` bits.
///
/// On overflow, frequencies are scaled down and the construction repeats;
/// this terminates and stays near-optimal.
fn huffman_code_lengths(freqs: &[u32], limit: u32) -> Vec<u8> {
    let n = freqs.len();
    let mut result = vec![0u8; n];
    let used: Vec<usize> = (0..n).filter(|&i| freqs[i] > 0).collect();
    if used.is_empty() {
        return result;
    }
    if used.len() == 1 {
        result[used[0]] = 1;
        return result;
    }
    let mut scaled: Vec<u64> = freqs.iter().map(|&f| f as u64).collect();
    loop {
        let mut heap: BinaryHeap<std::cmp::Reverse<(u64, i32)>> = BinaryHeap::new();
        for &s in &used {
            heap.push(std::cmp::Reverse((scaled[s], s as i32)));
        }
        let mut children: Vec<(i32, i32)> = Vec::with_capacity(used.len());
        let mut next_id = n as i32;
        while heap.len() > 1 {
            let a = heap.pop().unwrap().0;
            let b = heap.pop().unwrap().0;
            children.push((a.1, b.1));
            heap.push(std::cmp::Reverse((a.0 + b.0, next_id)));
            next_id += 1;
        }
        let root = heap.pop().unwrap().0 .1;
        let mut max_depth = 0u32;
        let mut stack = vec![(root, 0u32)];
        while let Some((id, depth)) = stack.pop() {
            if id < n as i32 {
                result[id as usize] = depth as u8;
                max_depth = max_depth.max(depth);
            } else {
                let (a, b) = children[(id - n as i32) as usize];
                stack.push((a, depth + 1));
                stack.push((b, depth + 1));
            }
        }
        if max_depth <= limit {
            return result;
        }
        for &s in &used {
            scaled[s] = (scaled[s] >> 1) | 1;
        }
    }
}

/// Canonical Huffman decode table (RFC 1951 §3.2.2).
#[derive(Debug)]
struct HuffTable {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl HuffTable {
    fn new(lengths: &[u8]) -> Result<Self, DeflateError> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            if l > 15 {
                return Err(DeflateError::InvalidHuffmanCode);
            }
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        // Over-subscription check.
        let mut left: i32 = 1;
        for &count in counts.iter().skip(1) {
            left <<= 1;
            left -= count as i32;
            if left < 0 {
                return Err(DeflateError::InvalidHuffmanCode);
            }
        }
        // Symbols sorted by (length, symbol value) — canonical order.
        let used = lengths.iter().filter(|&&l| l > 0).count();
        let mut symbols = vec![0u16; used];
        let mut offs = [0usize; 16];
        for len in 1..15usize {
            offs[len + 1] = offs[len] + counts[len] as usize;
        }
        for (sym, &l) in lengths.iter().enumerate() {
            if l > 0 {
                symbols[offs[l as usize]] = sym as u16;
                offs[l as usize] += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    /// Decode one symbol.
    ///
    /// `Ok(None)` means the input ran out mid-symbol (the caller pauses and
    /// retries); `Err` means the code is invalid.
    fn decode(&self, src: &mut BitSource<'_>) -> Result<Option<u16>, DeflateError> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..=15usize {
            code |= match src.bit() {
                Some(b) => b as i32,
                None => return Ok(None),
            };
            let count = self.counts[len] as i32;
            if code - first < count {
                return Ok(Some(self.symbols[(index + (code - first)) as usize]));
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(DeflateError::InvalidHuffmanCode)
    }
}

/// LSB-first bit reader that signals "out of input" with `None`, enabling
/// pause/resume at any symbol boundary.
struct BitSource<'a> {
    input: &'a [u8],
    pos: usize,
    bitbuf: u64,
    bitcnt: u32,
}

impl BitSource<'_> {
    /// Pull one byte into the accumulator; `None` when input is exhausted.
    fn byte(&mut self) -> Option<()> {
        if self.pos >= self.input.len() {
            return None;
        }
        self.bitbuf |= (self.input[self.pos] as u64) << self.bitcnt;
        self.pos += 1;
        self.bitcnt += 8;
        Some(())
    }

    /// Read a single bit.
    fn bit(&mut self) -> Option<u32> {
        if self.bitcnt == 0 {
            self.byte()?;
        }
        let b = (self.bitbuf & 1) as u32;
        self.bitbuf >>= 1;
        self.bitcnt -= 1;
        Some(b)
    }

    /// Read the low `n` bits (LSB first); `n <= 16`.
    fn bits(&mut self, n: u32) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        while self.bitcnt < n {
            self.byte()?;
        }
        let v = (self.bitbuf & ((1u64 << n) - 1)) as u32;
        self.bitbuf >>= n;
        self.bitcnt -= n;
        Some(v)
    }
}

// ---------------------------------------------------------------------------
// Encoder core (LZ77 + Huffman block emission)
// ---------------------------------------------------------------------------

/// Encoder input block size for Huffman levels.
const BLOCK_SIZE: usize = 65_536;

/// Stored blocks carry at most 65535 bytes.
const STORED_BLOCK_SIZE: usize = 65_535;

const HASH_BITS: u32 = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
const HASH_MASK: u64 = (HASH_SIZE - 1) as u64;

const NIL: u64 = u64::MAX;

#[derive(Debug, Clone, Copy)]
enum Token {
    Lit(u8),
    Match { len: u32, dist: u32 },
}

#[derive(Debug)]
struct EncoderCore {
    level: CompressionLevel,
    sink: BitSink,
    window: Vec<u8>,
    win_base: u64,
    head: Vec<u64>,
    prev: Vec<u64>,
    blockbuf: Vec<u8>,
    tokens: Vec<Token>,
    blocks_emitted: u64,
    ended: bool,
}

impl EncoderCore {
    fn new(level: CompressionLevel) -> Self {
        Self {
            level,
            sink: BitSink::new(),
            window: Vec::new(),
            win_base: 0,
            head: vec![NIL; HASH_SIZE],
            prev: vec![NIL; WINDOW_SIZE],
            blockbuf: Vec::with_capacity(BLOCK_SIZE),
            tokens: Vec::new(),
            blocks_emitted: 0,
            ended: false,
        }
    }

    fn block_limit(&self) -> usize {
        if self.level.is_stored() {
            STORED_BLOCK_SIZE
        } else {
            BLOCK_SIZE
        }
    }

    fn push(&mut self, data: &[u8]) {
        let mut rest = data;
        while !rest.is_empty() {
            let room = self.block_limit() - self.blockbuf.len();
            let take = min(room, rest.len());
            self.blockbuf.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.blockbuf.len() >= self.block_limit() {
                self.flush_block(false);
            }
        }
    }

    fn finish(&mut self) {
        if self.ended {
            return;
        }
        if !self.blockbuf.is_empty() {
            self.flush_block(true);
        } else {
            self.emit_empty_final_block();
        }
        self.ended = true;
        self.sink.align();
    }

    fn emit_empty_final_block(&mut self) {
        if self.level.is_stored() {
            self.sink.put(1, 1); // BFINAL
            self.sink.put(0, 2); // BTYPE 00
            self.sink.align();
            self.sink.put(0, 8);
            self.sink.put(0, 8);
            self.sink.put(0xFF, 8);
            self.sink.put(0xFF, 8);
        } else {
            // Fixed-Huffman final block containing only the end-of-block code.
            self.sink.put(1, 1); // BFINAL
            self.sink.put(1, 2); // BTYPE 01
            let codes = canonical_codes(&FIXED_LIT_LENGTHS);
            self.sink.put_code(codes[256], FIXED_LIT_LENGTHS[256]);
        }
        self.blocks_emitted += 1;
    }

    fn flush_block(&mut self, final_block: bool) {
        let take = if final_block {
            self.blockbuf.len()
        } else {
            self.block_limit()
        };
        if self.level.is_stored() {
            emit_stored(&mut self.sink, final_block, &self.blockbuf[..take]);
            self.blockbuf.drain(..take);
            self.blocks_emitted += 1;
            return;
        }
        let block_pos = self.win_base + self.window.len() as u64;
        self.window.extend_from_slice(&self.blockbuf[..take]);
        let block_end = self.win_base + self.window.len() as u64;
        self.blockbuf.drain(..take);
        self.tokenize(block_pos, block_end);
        self.emit_huff_block(final_block);
        self.blocks_emitted += 1;
        self.drain_window();
    }

    fn drain_window(&mut self) {
        if self.window.len() > 2 * WINDOW_SIZE {
            let drop = self.window.len() - WINDOW_SIZE;
            self.window.drain(..drop);
            self.win_base += drop as u64;
        }
    }

    fn byte_at(&self, logical: u64) -> u8 {
        self.window[(logical - self.win_base) as usize]
    }

    fn insert_pos(&mut self, p: u64) {
        let pi = (p - self.win_base) as usize;
        if pi + 3 > self.window.len() {
            return;
        }
        let h = hash3(self.window[pi], self.window[pi + 1], self.window[pi + 2]) as usize;
        let slot = (p & HASH_MASK) as usize;
        self.prev[slot] = self.head[h];
        self.head[h] = p;
    }

    fn find_match(&self, p: u64, block_end: u64, max_chain: usize) -> (u32, u32) {
        let pi = (p - self.win_base) as usize;
        if pi + 3 > self.window.len() {
            return (0, 0);
        }
        let max_len = min(258usize, (block_end - p) as usize);
        if max_len < 3 {
            return (0, 0);
        }
        let h = hash3(self.window[pi], self.window[pi + 1], self.window[pi + 2]) as usize;
        let mut cur = self.head[h];
        let mut best_len = 2usize;
        let mut best_dist = 0u32;
        let mut chain = max_chain;
        while cur != NIL && chain > 0 {
            if cur >= p {
                cur = self.prev[(cur & HASH_MASK) as usize];
                chain -= 1;
                continue;
            }
            if cur < self.win_base {
                break;
            }
            let dist = (p - cur) as u32;
            if dist as usize > WINDOW_SIZE {
                break;
            }
            let ci = (cur - self.win_base) as usize;
            let mut l = 0usize;
            while l < max_len && self.window[ci + l] == self.window[pi + l] {
                l += 1;
            }
            if l > best_len {
                best_len = l;
                best_dist = dist;
                if l >= max_len {
                    break;
                }
            }
            cur = self.prev[(cur & HASH_MASK) as usize];
            chain -= 1;
        }
        (best_len as u32, best_dist)
    }

    fn tokenize(&mut self, mut p: u64, block_end: u64) {
        let (max_chain, lazy) = match self.level {
            CompressionLevel::Stored => (0, false),
            CompressionLevel::Fast => (16, false),
            CompressionLevel::Normal => (64, true),
            CompressionLevel::Best => (256, true),
        };
        self.tokens.clear();
        while p + 3 <= block_end {
            let (best_len, best_dist) = self.find_match(p, block_end, max_chain);
            let mut inserted_here = false;
            if lazy && best_len >= 3 && p + 4 <= block_end {
                self.insert_pos(p);
                inserted_here = true;
                let (len2, _) = self.find_match(p + 1, block_end, max_chain);
                if len2 > best_len {
                    // Emit a literal; the outer loop re-examines p + 1 and
                    // takes the longer match there.
                    self.tokens.push(Token::Lit(self.byte_at(p)));
                    p += 1;
                    continue;
                }
            }
            if best_len >= 3 {
                self.tokens.push(Token::Match {
                    len: best_len,
                    dist: best_dist,
                });
                let mut i = if inserted_here { p + 1 } else { p };
                let end = p + best_len as u64;
                while i < end && i + 3 <= block_end {
                    self.insert_pos(i);
                    i += 1;
                }
                p = end;
            } else {
                if !inserted_here {
                    self.insert_pos(p);
                }
                self.tokens.push(Token::Lit(self.byte_at(p)));
                p += 1;
            }
        }
        while p < block_end {
            self.tokens.push(Token::Lit(self.byte_at(p)));
            p += 1;
        }
    }

    fn emit_huff_block(&mut self, final_block: bool) {
        let mut lit_freq = [0u32; 286];
        let mut dist_freq = [0u32; 30];
        let mut extra_bits: u64 = 0;
        for tok in &self.tokens {
            match *tok {
                Token::Lit(b) => lit_freq[b as usize] += 1,
                Token::Match { len, dist } => {
                    let (ls, le) = length_symbol(len);
                    lit_freq[ls as usize] += 1;
                    extra_bits += le as u64;
                    let (ds, de) = dist_symbol(dist);
                    dist_freq[ds as usize] += 1;
                    extra_bits += de as u64;
                }
            }
        }
        lit_freq[256] += 1; // end-of-block

        let fixed_bits = 3u64
            + lit_freq
                .iter()
                .zip(FIXED_LIT_LENGTHS.iter())
                .map(|(f, l)| *f as u64 * *l as u64)
                .sum::<u64>()
            + dist_freq.iter().map(|f| *f as u64 * 5).sum::<u64>()
            + extra_bits;

        let mut plan = None;
        if self.level == CompressionLevel::Best && !self.tokens.is_empty() {
            let candidate = plan_dynamic(&lit_freq, &dist_freq, extra_bits);
            if candidate.total_bits < fixed_bits {
                plan = Some(candidate);
            }
        }

        match plan {
            Some(plan) => {
                self.sink.put(final_block as u32, 1);
                self.sink.put(2, 2); // BTYPE 10
                self.sink.put((plan.hlit - 257) as u32, 5);
                self.sink.put((plan.hdist - 1) as u32, 5);
                self.sink.put((plan.hc_len - 4) as u32, 4);
                let meta_codes = canonical_codes(&plan.meta_lengths);
                for k in 0..plan.hc_len {
                    self.sink.put(plan.meta_lengths[ORDER[k]] as u32, 3);
                }
                for (sym, extra_n, extra_v) in &plan.items {
                    self.sink
                        .put_code(meta_codes[*sym as usize], plan.meta_lengths[*sym as usize]);
                    self.sink.put(*extra_v, *extra_n);
                }
                emit_tokens(
                    &mut self.sink,
                    &self.tokens,
                    &plan.lit_codes,
                    &plan.lit_lengths,
                    &plan.dist_codes,
                    &plan.dist_lengths,
                );
            }
            None => {
                self.sink.put(final_block as u32, 1);
                self.sink.put(1, 2); // BTYPE 01
                let lit_codes = canonical_codes(&FIXED_LIT_LENGTHS);
                let dist_codes = canonical_codes(&FIXED_DIST_LENGTHS);
                emit_tokens(
                    &mut self.sink,
                    &self.tokens,
                    &lit_codes,
                    &FIXED_LIT_LENGTHS,
                    &dist_codes,
                    &FIXED_DIST_LENGTHS,
                );
            }
        }
    }
}

/// Emit the token stream plus the end-of-block code.
fn emit_tokens(
    sink: &mut BitSink,
    tokens: &[Token],
    lit_codes: &[u16],
    lit_lengths: &[u8],
    dist_codes: &[u16],
    dist_lengths: &[u8],
) {
    for tok in tokens {
        match *tok {
            Token::Lit(b) => {
                sink.put_code(lit_codes[b as usize], lit_lengths[b as usize]);
            }
            Token::Match { len, dist } => {
                let (ls, le) = length_symbol(len);
                sink.put_code(lit_codes[ls as usize], lit_lengths[ls as usize]);
                if le > 0 {
                    sink.put(len - length_base(ls), le);
                }
                let (ds, de) = dist_symbol(dist);
                sink.put_code(dist_codes[ds as usize], dist_lengths[ds as usize]);
                if de > 0 {
                    sink.put(dist - dist_base(ds), de);
                }
            }
        }
    }
    sink.put_code(lit_codes[256], lit_lengths[256]);
}

/// Emit one stored block (BTYPE 00).
fn emit_stored(sink: &mut BitSink, final_block: bool, data: &[u8]) {
    let len = data.len();
    debug_assert!(len <= STORED_BLOCK_SIZE);
    sink.put(final_block as u32, 1);
    sink.put(0, 2);
    sink.align();
    sink.put((len & 0xFF) as u32, 8);
    sink.put((len >> 8) as u32, 8);
    sink.put((!len & 0xFF) as u32, 8);
    sink.put(((!len >> 8) & 0xFF) as u32, 8);
    for &b in data {
        sink.put(b as u32, 8);
    }
}

/// Hash three bytes into a 15-bit bucket.
fn hash3(a: u8, b: u8, c: u8) -> u64 {
    ((a as u64) << 10 ^ (b as u64) << 5 ^ (c as u64)) & HASH_MASK
}

fn length_base(sym: u16) -> u32 {
    LEN_BASE[(sym - 257) as usize]
}

fn dist_base(sym: u16) -> u32 {
    DIST_BASE[sym as usize]
}

/// Map a match length (3..=258) to its DEFLATE symbol and extra-bit count.
fn length_symbol(len: u32) -> (u16, u32) {
    debug_assert!((3..=258).contains(&len));
    let mut idx = LEN_BASE.len() - 1;
    while LEN_BASE[idx] > len {
        idx -= 1;
    }
    ((257 + idx) as u16, LEN_EXTRA[idx])
}

/// Map a match distance (1..=32768) to its DEFLATE symbol and extra-bit count.
fn dist_symbol(dist: u32) -> (u16, u32) {
    debug_assert!(dist >= 1 && dist as usize <= WINDOW_SIZE);
    let mut idx = DIST_BASE.len() - 1;
    while DIST_BASE[idx] > dist {
        idx -= 1;
    }
    (idx as u16, DIST_EXTRA[idx])
}

/// A fully planned dynamic block (code lengths plus encoded header).
struct DynamicPlan {
    lit_lengths: Vec<u8>,
    dist_lengths: Vec<u8>,
    lit_codes: Vec<u16>,
    dist_codes: Vec<u16>,
    hlit: usize,
    hdist: usize,
    items: Vec<(u8, u32, u32)>,
    meta_lengths: Vec<u8>,
    hc_len: usize,
    total_bits: u64,
}

fn plan_dynamic(lit_freq: &[u32; 286], dist_freq: &[u32; 30], extra_bits: u64) -> DynamicPlan {
    let lit_lengths_all = huffman_code_lengths(lit_freq, 15);
    let mut hlit = lit_lengths_all.len();
    while hlit > 257 && lit_lengths_all[hlit - 1] == 0 {
        hlit -= 1;
    }
    let lit_lengths = lit_lengths_all[..hlit].to_vec();

    let any_dist = dist_freq.iter().any(|&f| f > 0);
    let dist_lengths_all = if any_dist {
        huffman_code_lengths(dist_freq, 15)
    } else {
        // RFC 1951: "one distance code of zero bits" is invalid; a single
        // 1-bit code is the sanctioned degenerate case.
        vec![1u8; 1]
    };
    let mut hdist = dist_lengths_all.len();
    while hdist > 1 && dist_lengths_all[hdist - 1] == 0 {
        hdist -= 1;
    }
    let dist_lengths = dist_lengths_all[..hdist].to_vec();

    // Combined code-length sequence, RLE-encoded over the code-length alphabet.
    let mut all: Vec<u8> = lit_lengths.clone();
    all.extend_from_slice(&dist_lengths);
    let mut items: Vec<(u8, u32, u32)> = Vec::new();
    let mut i = 0usize;
    while i < all.len() {
        let v = all[i];
        let mut run = 1usize;
        while i + run < all.len() && all[i + run] == v {
            run += 1;
        }
        let mut left = run;
        if v == 0 {
            while left >= 3 {
                if left >= 11 {
                    let take = min(left, 138);
                    items.push((18, 7, (take - 11) as u32));
                    left -= take;
                } else {
                    let take = min(left, 10);
                    items.push((17, 3, (take - 3) as u32));
                    left -= take;
                }
            }
        } else {
            items.push((v, 0, 0));
            left -= 1;
            while left >= 3 {
                let take = min(left, 6);
                items.push((16, 2, (take - 3) as u32));
                left -= take;
            }
        }
        for _ in 0..left {
            items.push((v, 0, 0));
        }
        i += run;
    }

    let mut meta_freq = [0u32; 19];
    for (sym, _, _) in &items {
        meta_freq[*sym as usize] += 1;
    }
    let meta_lengths = huffman_code_lengths(&meta_freq, 7);
    let mut hc_len = 19usize;
    while hc_len > 4 && meta_lengths[ORDER[hc_len - 1]] == 0 {
        hc_len -= 1;
    }

    let header_bits = 3u64
        + 5
        + 5
        + 4
        + (hc_len as u64) * 3
        + items
            .iter()
            .map(|(sym, en, _)| meta_lengths[*sym as usize] as u64 + *en as u64)
            .sum::<u64>();

    let token_bits = lit_freq
        .iter()
        .zip(lit_lengths_all.iter())
        .map(|(f, l)| *f as u64 * *l as u64)
        .sum::<u64>()
        + dist_freq
            .iter()
            .zip(dist_lengths_all.iter())
            .map(|(f, l)| *f as u64 * *l as u64)
            .sum::<u64>()
        + extra_bits;

    let lit_codes = canonical_codes(&lit_lengths);
    let dist_codes = canonical_codes(&dist_lengths);

    DynamicPlan {
        lit_lengths,
        dist_lengths,
        lit_codes,
        dist_codes,
        hlit,
        hdist,
        items,
        meta_lengths,
        hc_len,
        total_bits: header_bits + token_bits,
    }
}

// ---------------------------------------------------------------------------
// RFC 1951 constant tables
// ---------------------------------------------------------------------------

/// Fixed literal/length code lengths (RFC 1951 §3.2.6).
const FIXED_LIT_LENGTHS: [u8; 288] = {
    let mut t = [0u8; 288];
    let mut i = 0;
    while i < 144 {
        t[i] = 8;
        i += 1;
    }
    while i < 256 {
        t[i] = 9;
        i += 1;
    }
    while i < 280 {
        t[i] = 7;
        i += 1;
    }
    while i < 288 {
        t[i] = 8;
        i += 1;
    }
    t
};

/// Fixed distance code lengths (RFC 1951 §3.2.6).
const FIXED_DIST_LENGTHS: [u8; 32] = [5; 32];

/// Length symbol bases (RFC 1951 §3.2.5), indexed by symbol − 257.
const LEN_BASE: [u32; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];

/// Length symbol extra-bit counts.
const LEN_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

/// Distance symbol bases (RFC 1951 §3.2.5).
const DIST_BASE: [u32; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];

/// Distance symbol extra-bit counts.
const DIST_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Code-length alphabet order (RFC 1951 §3.2.7).
const ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];
