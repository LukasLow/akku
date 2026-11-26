//! Safe, refcounted byte buffers with O(1) zero-copy slicing, a growable
//! builder, and a bounds-checked binary reader.
//!
//! [`Bytes`] is an immutable view over shared storage (`Arc<[u8]>`) or static
//! data; cloning and [`Bytes::slice`] are O(1) and never copy. [`BytesMut`]
//! accumulates bytes with explicit big-/little-endian `put_*` helpers and can
//! [`BytesMut::freeze`] into an immutable [`Bytes`]. [`Cursor`] reads typed
//! values back and returns [`BytesError`] on underflow instead of panicking.
//!
//! # Quick start
//!
//! ```
//! use akku::bytes::{Bytes, BytesMut, Cursor};
//!
//! let mut buf = BytesMut::new();
//! buf.put_u16(0x1234);
//! buf.put_u16_le(0x1234);
//! let frozen = buf.freeze();
//! assert_eq!(frozen.len(), 4);
//!
//! let mut reader = Cursor::new(&frozen);
//! assert_eq!(reader.get_u16().unwrap(), 0x1234);
//! assert_eq!(reader.get_u16_le().unwrap(), 0x1234);
//! assert!(reader.is_empty());
//! ```
//!
//! # Varints
//!
//! [`BytesMut::put_uvarint`] / [`Cursor::get_uvarint`] (and the zigzag
//! `put_varint` / `get_varint`) read and write LEB128 varints; the
//! [`varint`] submodule has the slice- and `Vec`-based functions.
//!
//! ```
//! use akku::bytes::{BytesMut, Cursor};
//!
//! let mut buf = BytesMut::new();
//! buf.put_uvarint(300);
//! buf.put_varint(-1);
//! let mut reader = Cursor::new(&buf);
//! assert_eq!(reader.get_uvarint(), Ok(300));
//! assert_eq!(reader.get_varint(), Ok(-1));
//! ```
//!
//! # Zero-copy slicing
//!
//! ```
//! use akku::bytes::Bytes;
//!
//! let base = Bytes::from_static(b"hello akku");
//! let word = base.slice(6..); // O(1): shares storage, no allocation
//! assert_eq!(word.as_slice(), b"akku");
//! ```

pub mod varint;

use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::{Bound, Deref, DerefMut, RangeBounds};
use std::sync::Arc;

/// Returned when a reader wanted more bytes than the buffer had left.
///
/// `needed` is the number of bytes the operation required as a whole;
/// `remaining` is how many bytes were actually left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BytesError {
    /// Number of bytes the attempted operation needed in total.
    pub needed: usize,
    /// Number of bytes that were actually remaining.
    pub remaining: usize,
}

impl fmt::Display for BytesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unexpected end of buffer: wanted {} byte(s), {} remaining",
            self.needed, self.remaining,
        )
    }
}

impl std::error::Error for BytesError {}

/// Storage behind [`Bytes`]: static data or an owned shared allocation.
#[derive(Clone)]
enum Backing {
    /// Compile-time data; never freed and never allocated.
    Static(&'static [u8]),
    /// Shared heap allocation; freed when the last clone disappears.
    Shared(Arc<[u8]>),
}

/// An immutable, refcounted byte buffer with O(1) slicing.
///
/// `Bytes` holds either static data ([`Bytes::from_static`] allocates
/// nothing) or one `Arc<[u8]>` shared with every clone. Slicing
/// ([`Bytes::slice`], [`Bytes::split_to`], [`Bytes::split_off`], [`Bytes::truncate`])
/// only moves the `start..end` window over the shared storage and never
/// copies bytes.
///
/// ```
/// use akku::bytes::Bytes;
///
/// let base = Bytes::from_static(b"abcdef");
/// let head = base.slice(..2);
/// assert_eq!(head.as_slice(), b"ab");
/// assert_eq!(base.as_slice(), b"abcdef");
/// ```
pub struct Bytes {
    backing: Backing,
    start: usize,
    end: usize,
}

impl Bytes {
    /// Creates an empty buffer backed by a static empty slice.
    pub fn new() -> Self {
        Self {
            backing: Backing::Static(&[]),
            start: 0,
            end: 0,
        }
    }

    /// Creates a buffer over `&'static` data without touching the heap.
    pub fn from_static(data: &'static [u8]) -> Self {
        Self {
            backing: Backing::Static(data),
            start: 0,
            end: data.len(),
        }
    }

    /// Copies `data` into one fresh shared allocation.
    pub fn copy_from_slice(data: &[u8]) -> Self {
        Self {
            backing: Backing::Shared(data.to_vec().into()),
            start: 0,
            end: data.len(),
        }
    }

    /// Returns the number of visible bytes.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Returns whether the visible window is empty.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Returns the visible bytes as a slice.
    pub fn as_slice(&self) -> &[u8] {
        let storage: &[u8] = match &self.backing {
            Backing::Static(data) => data,
            Backing::Shared(data) => data,
        };
        &storage[self.start..self.end]
    }

    /// Returns an O(1) zero-copy subslice whose bounds are relative to this
    /// window.
    ///
    /// Panics when `range` is not fully inside `0..self.len()`.
    pub fn slice(&self, range: impl RangeBounds<usize>) -> Bytes {
        let (start, end) = bounds_of(range, self.len());
        Bytes {
            backing: self.backing.clone(),
            start: self.start + start,
            end: self.start + end,
        }
    }

    /// Removes and returns the first `at` bytes; this window shrinks to the
    /// remainder.
    ///
    /// Taking everything leaves this window empty and releases its share of
    /// the storage.
    ///
    /// Panics when `at > self.len()`.
    pub fn split_to(&mut self, at: usize) -> Bytes {
        assert!(
            at <= self.len(),
            "split_to: {at} exceeds length {}",
            self.len()
        );
        let head = Bytes {
            backing: self.backing.clone(),
            start: self.start,
            end: self.start + at,
        };
        self.start += at;
        if self.is_empty() {
            self.reset_empty();
        }
        head
    }

    /// Keeps the first `at` bytes in this window and returns the rest.
    ///
    /// Peeling off zero bytes leaves this window empty and releases its share
    /// of the storage.
    ///
    /// Panics when `at > self.len()`.
    pub fn split_off(&mut self, at: usize) -> Bytes {
        assert!(
            at <= self.len(),
            "split_off: {at} exceeds length {}",
            self.len()
        );
        let tail = Bytes {
            backing: self.backing.clone(),
            start: self.start + at,
            end: self.end,
        };
        self.end = self.start + at;
        if self.is_empty() {
            self.reset_empty();
        }
        tail
    }

    /// Shortens this window to `len` bytes; longer `len` values are no-ops.
    ///
    /// Truncating to zero releases the shared storage, so an emptied buffer
    /// never pins a large allocation.
    pub fn truncate(&mut self, len: usize) {
        if len < self.len() {
            self.end = self.start + len;
        }
        if self.is_empty() {
            self.reset_empty();
        }
    }

    /// Empties this window and releases the shared storage; clones keep it
    /// alive for as long as they exist.
    pub fn clear(&mut self) {
        self.reset_empty();
    }

    /// Switches to the static empty slice so an empty window holds no `Arc`.
    fn reset_empty(&mut self) {
        self.backing = Backing::Static(&[]);
        self.start = 0;
        self.end = 0;
    }

    /// Copies the visible bytes into a fresh [`Vec`].
    pub fn to_vec(&self) -> Vec<u8> {
        self.as_slice().to_vec()
    }
}

/// Translates a `RangeBounds<usize>` into an inclusive `(start, end)` pair
/// within `0..len`, panicking on out-of-bounds input.
fn bounds_of(range: impl RangeBounds<usize>, len: usize) -> (usize, usize) {
    let start = match range.start_bound() {
        Bound::Included(&start) => start,
        Bound::Excluded(&start) => start.saturating_add(1),
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound() {
        Bound::Included(&end) => end.saturating_add(1),
        Bound::Excluded(&end) => end,
        Bound::Unbounded => len,
    };
    assert!(
        start <= end && end <= len,
        "slice range {start}..{end} out of bounds for length {len}",
    );
    (start, end)
}

impl Default for Bytes {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for Bytes {
    fn clone(&self) -> Self {
        Self {
            backing: self.backing.clone(),
            start: self.start,
            end: self.end,
        }
    }
}

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.as_slice().iter()).finish()
    }
}

impl PartialEq for Bytes {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Eq for Bytes {}

impl PartialOrd for Bytes {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Bytes {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

impl Hash for Bytes {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Deref for Bytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl PartialEq<[u8]> for Bytes {
    fn eq(&self, other: &[u8]) -> bool {
        self.as_slice() == other
    }
}

impl PartialEq<&[u8]> for Bytes {
    fn eq(&self, other: &&[u8]) -> bool {
        self.as_slice() == *other
    }
}

impl PartialEq<Vec<u8>> for Bytes {
    fn eq(&self, other: &Vec<u8>) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(data: Vec<u8>) -> Self {
        let end = data.len();
        Self {
            backing: Backing::Shared(data.into()),
            start: 0,
            end,
        }
    }
}

impl From<&'static [u8]> for Bytes {
    fn from(data: &'static [u8]) -> Self {
        Self::from_static(data)
    }
}

impl From<&'static str> for Bytes {
    fn from(data: &'static str) -> Self {
        Self {
            backing: Backing::Static(data.as_bytes()),
            start: 0,
            end: data.len(),
        }
    }
}

impl From<String> for Bytes {
    fn from(data: String) -> Self {
        let bytes = data.into_bytes();
        let end = bytes.len();
        Self {
            backing: Backing::Shared(bytes.into()),
            start: 0,
            end,
        }
    }
}

/// A growable, mutable byte buffer with explicit endian `put_*` helpers.
///
/// Cloning copies the bytes (unlike [`Bytes`]). [`BytesMut::freeze`] performs a
/// one-time conversion of the buffer into one shared `Arc<[u8]>` allocation
/// without copying the visible bytes; after that, cloning and slicing the
/// resulting [`Bytes`] are O(1) and zero-copy.
///
/// ```
/// use akku::bytes::BytesMut;
/// use std::io::Write;
///
/// let mut buf = BytesMut::with_capacity(8);
/// write!(buf, "hello ")?;
/// buf.extend_from_slice(b"world");
/// assert_eq!(buf.as_ref(), b"hello world");
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BytesMut {
    buf: Vec<u8>,
}

impl BytesMut {
    /// Creates an empty buffer.
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Creates an empty buffer with room for at least `capacity` bytes.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
        }
    }

    /// Returns the number of buffered bytes.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Returns whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Returns the number of bytes that fit without reallocating.
    pub fn capacity(&self) -> usize {
        self.buf.capacity()
    }

    /// Reserves space for at least `additional` more bytes.
    pub fn reserve(&mut self, additional: usize) {
        self.buf.reserve(additional);
    }

    /// Removes every byte.
    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// Shortens the buffer to `len` bytes; longer values are no-ops.
    pub fn truncate(&mut self, len: usize) {
        self.buf.truncate(len);
    }

    /// Appends `data`, growing the buffer as needed.
    pub fn extend_from_slice(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Appends `data` (alias of [`BytesMut::extend_from_slice`], BufMut
    /// style).
    pub fn put_slice(&mut self, data: &[u8]) {
        self.extend_from_slice(data);
    }

    /// Appends one byte.
    pub fn put_u8(&mut self, value: u8) {
        self.buf.push(value);
    }

    /// Appends `value` as an LEB128 varint (1 to 10 bytes).
    pub fn put_uvarint(&mut self, value: u64) {
        varint::append_u64(&mut self.buf, value);
    }

    /// Appends `value` as a zigzag varint (1 to 10 bytes).
    pub fn put_varint(&mut self, value: i64) {
        varint::append_i64(&mut self.buf, value);
    }

    /// Removes and returns the first `at` bytes; this buffer keeps the rest.
    ///
    /// Panics when `at > self.len()`.
    pub fn split_to(&mut self, at: usize) -> BytesMut {
        assert!(
            at <= self.buf.len(),
            "split_to: {at} exceeds length {}",
            self.buf.len()
        );
        BytesMut {
            buf: self.buf.drain(..at).collect(),
        }
    }

    /// Keeps the first `at` bytes in this buffer and returns the rest.
    ///
    /// Panics when `at > self.len()`.
    pub fn split_off(&mut self, at: usize) -> BytesMut {
        assert!(
            at <= self.buf.len(),
            "split_off: {at} exceeds length {}",
            self.buf.len()
        );
        BytesMut {
            buf: self.buf.split_off(at),
        }
    }

    /// Converts the buffer into an immutable [`Bytes`] backed by one shared
    /// allocation.
    ///
    /// This is a one-time conversion: the buffer's `Vec<u8>` becomes an
    /// `Arc<[u8]>` without copying the visible bytes. Afterwards, cloning and
    /// slicing the frozen [`Bytes`] are O(1) and zero-copy, which makes
    /// `freeze` cheap to call once per completed message and expensive to call
    /// on every mutation.
    ///
    /// ```
    /// use akku::bytes::BytesMut;
    ///
    /// let mut buf = BytesMut::new();
    /// buf.extend_from_slice(b"share me");
    /// let frozen = buf.freeze();
    /// assert_eq!(frozen.as_slice(), b"share me");
    ///
    /// let second = frozen.clone();
    /// let part = second.slice(6..);
    /// assert_eq!(part.as_slice(), b"me");
    /// ```
    pub fn freeze(self) -> Bytes {
        let end = self.buf.len();
        Bytes {
            backing: Backing::Shared(self.buf.into()),
            start: 0,
            end,
        }
    }
}

/// Generates paired big-/little-endian `put_*` methods on [`BytesMut`].
macro_rules! put_methods {
    ($(($name:ident, $name_le:ident, $ty:ty, $doc_be:expr, $doc_le:expr);)+) => {
        $(
            #[doc = $doc_be]
            pub fn $name(&mut self, value: $ty) {
                self.buf.extend_from_slice(&value.to_be_bytes());
            }

            #[doc = $doc_le]
            pub fn $name_le(&mut self, value: $ty) {
                self.buf.extend_from_slice(&value.to_le_bytes());
            }
        )+
    };
}

impl BytesMut {
    put_methods! {
        (put_u16, put_u16_le, u16, "Appends a `u16` in big-endian order.", "Appends a `u16` in little-endian order.");
        (put_u32, put_u32_le, u32, "Appends a `u32` in big-endian order.", "Appends a `u32` in little-endian order.");
        (put_u64, put_u64_le, u64, "Appends a `u64` in big-endian order.", "Appends a `u64` in little-endian order.");
        (put_u128, put_u128_le, u128, "Appends a `u128` in big-endian order.", "Appends a `u128` in little-endian order.");
        (put_i16, put_i16_le, i16, "Appends an `i16` in big-endian order.", "Appends an `i16` in little-endian order.");
        (put_i32, put_i32_le, i32, "Appends an `i32` in big-endian order.", "Appends an `i32` in little-endian order.");
        (put_i64, put_i64_le, i64, "Appends an `i64` in big-endian order.", "Appends an `i64` in little-endian order.");
        (put_i128, put_i128_le, i128, "Appends an `i128` in big-endian order.", "Appends an `i128` in little-endian order.");
        (put_f32, put_f32_le, f32, "Appends an `f32` (bit pattern) in big-endian order.", "Appends an `f32` (bit pattern) in little-endian order.");
        (put_f64, put_f64_le, f64, "Appends an `f64` (bit pattern) in big-endian order.", "Appends an `f64` (bit pattern) in little-endian order.");
    }
}

impl AsRef<[u8]> for BytesMut {
    fn as_ref(&self) -> &[u8] {
        &self.buf
    }
}

impl AsMut<[u8]> for BytesMut {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.buf
    }
}

impl Deref for BytesMut {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.buf
    }
}

impl DerefMut for BytesMut {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.buf
    }
}

impl std::io::Write for BytesMut {
    /// Appends `buf` and returns its length; the buffer never fills up
    /// because it grows on demand.
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A bounds-checked reader over a byte slice that returns
/// [`BytesError`] instead of panicking on underflow.
///
/// ```
/// use akku::bytes::{BytesError, Cursor};
///
/// let mut reader = Cursor::new(b"\x00\x01");
/// assert_eq!(reader.remaining(), 2);
/// assert_eq!(reader.get_u16().unwrap(), 1);
/// assert!(reader.get_u8().is_err());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// Creates a reader positioned at the first byte of `data`.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Returns the number of bytes not yet consumed.
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    /// Returns how many bytes have been consumed so far.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Returns whether all bytes are consumed.
    pub fn is_empty(&self) -> bool {
        self.pos == self.data.len()
    }

    /// Reads the next byte without advancing.
    pub fn peek_u8(&self) -> Result<u8, BytesError> {
        if self.is_empty() {
            return Err(BytesError {
                needed: 1,
                remaining: 0,
            });
        }
        Ok(self.data[self.pos])
    }

    /// Reads and consumes one byte.
    pub fn get_u8(&mut self) -> Result<u8, BytesError> {
        let value = self.peek_u8()?;
        self.pos += 1;
        Ok(value)
    }

    /// Reads and consumes `len` bytes, returned as a slice borrow.
    pub fn get_slice(&mut self, len: usize) -> Result<&'a [u8], BytesError> {
        if self.remaining() < len {
            return Err(BytesError {
                needed: len,
                remaining: self.remaining(),
            });
        }
        let slice = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    /// Skips `cnt` bytes.
    pub fn advance(&mut self, cnt: usize) -> Result<(), BytesError> {
        self.get_slice(cnt).map(|_| ())
    }

    /// Reads the next `N` bytes into a fixed array.
    fn fill<const N: usize>(&mut self) -> Result<[u8; N], BytesError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.get_slice(N)?);
        Ok(out)
    }

    /// Reads an LEB128 varint.
    ///
    /// On error the position is unchanged. Overlong encodings are accepted;
    /// see [`varint::decode_u64_strict`].
    pub fn get_uvarint(&mut self) -> Result<u64, varint::VarintError> {
        let (value, len) = varint::decode_u64(&self.data[self.pos..])?;
        self.pos += len;
        Ok(value)
    }

    /// Reads a zigzag varint; on error the position is unchanged.
    pub fn get_varint(&mut self) -> Result<i64, varint::VarintError> {
        let (value, len) = varint::decode_i64(&self.data[self.pos..])?;
        self.pos += len;
        Ok(value)
    }

    /// Reads an `f32` (bit pattern) in big-endian order.
    pub fn get_f32(&mut self) -> Result<f32, BytesError> {
        Ok(f32::from_bits(u32::from_be_bytes(self.fill()?)))
    }

    /// Reads an `f32` (bit pattern) in little-endian order.
    pub fn get_f32_le(&mut self) -> Result<f32, BytesError> {
        Ok(f32::from_bits(u32::from_le_bytes(self.fill()?)))
    }

    /// Reads an `f64` (bit pattern) in big-endian order.
    pub fn get_f64(&mut self) -> Result<f64, BytesError> {
        Ok(f64::from_bits(u64::from_be_bytes(self.fill()?)))
    }

    /// Reads an `f64` (bit pattern) in little-endian order.
    pub fn get_f64_le(&mut self) -> Result<f64, BytesError> {
        Ok(f64::from_bits(u64::from_le_bytes(self.fill()?)))
    }
}

/// Generates paired big-/little-endian `get_*` methods on [`Cursor`].
macro_rules! get_methods {
    ($(($name:ident, $name_le:ident, $ty:ty, $be:ident, $le:ident, $width:literal, $doc_be:expr, $doc_le:expr);)+) => {
        $(
            #[doc = $doc_be]
            pub fn $name(&mut self) -> Result<$ty, BytesError> {
                Ok(<$ty>::$be(self.fill::<$width>()?))
            }

            #[doc = $doc_le]
            pub fn $name_le(&mut self) -> Result<$ty, BytesError> {
                Ok(<$ty>::$le(self.fill::<$width>()?))
            }
        )+
    };
}

impl<'a> Cursor<'a> {
    get_methods! {
        (get_u16, get_u16_le, u16, from_be_bytes, from_le_bytes, 2, "Reads a `u16` in big-endian order.", "Reads a `u16` in little-endian order.");
        (get_u32, get_u32_le, u32, from_be_bytes, from_le_bytes, 4, "Reads a `u32` in big-endian order.", "Reads a `u32` in little-endian order.");
        (get_u64, get_u64_le, u64, from_be_bytes, from_le_bytes, 8, "Reads a `u64` in big-endian order.", "Reads a `u64` in little-endian order.");
        (get_u128, get_u128_le, u128, from_be_bytes, from_le_bytes, 16, "Reads a `u128` in big-endian order.", "Reads a `u128` in little-endian order.");
        (get_i16, get_i16_le, i16, from_be_bytes, from_le_bytes, 2, "Reads an `i16` in big-endian order.", "Reads an `i16` in little-endian order.");
        (get_i32, get_i32_le, i32, from_be_bytes, from_le_bytes, 4, "Reads an `i32` in big-endian order.", "Reads an `i32` in little-endian order.");
        (get_i64, get_i64_le, i64, from_be_bytes, from_le_bytes, 8, "Reads an `i64` in big-endian order.", "Reads an `i64` in little-endian order.");
        (get_i128, get_i128_le, i128, from_be_bytes, from_le_bytes, 16, "Reads an `i128` in big-endian order.", "Reads an `i128` in little-endian order.");
    }
}
