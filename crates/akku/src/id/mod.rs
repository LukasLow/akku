//! Identifiers without external dependencies: UUID v4/v7 (RFC 9562), ULID
//! and NanoID.
//!
//! # Entropy
//!
//! On Unix-like systems (including macOS) [`Uuid::v4`], [`Uuid::v7`],
//! [`Ulid::new`] and the [`nanoid`] family read random bytes from
//! `/dev/urandom` (`std::fs::File`), opened fresh for each call. If
//! `/dev/urandom` cannot be opened or read, a documented software fallback
//! kicks in: a splitmix64 PRNG seeded per process from
//! `std::collections::hash_map::RandomState` hashing, `SystemTime` nanos
//! since the epoch and the process id, advanced by an `AtomicU64` counter so
//! concurrent threads never reuse a state. The fallback is **not**
//! cryptographically secure; it guarantees uniqueness-style behavior for
//! identifiers on platforms without `/dev/urandom` (for example WASM
//! targets), which is all these constructors promise.
//!
//! Fully deterministic constructors ([`Uuid::v7_at`], [`Ulid::from_parts`],
//! [`Ulid::from_bytes`], [`Uuid::from_bytes`]) exist so tests can pin every
//! bit without touching entropy at all.
//!
//! # Quick start
//!
//! ```
//! use akku::id::{Ulid, Uuid};
//!
//! let id = Uuid::v4();
//! assert_eq!(id.version(), 4);
//! assert_eq!(Uuid::parse(&id.to_hyphenated()).unwrap(), id);
//!
//! let pinned = Uuid::v7_at(1_700_000_000_000, [0u8; 10]);
//! assert_eq!(pinned.timestamp_millis(), Some(1_700_000_000_000));
//!
//! let sorted = Ulid::from_parts(0, 1);
//! assert_eq!(sorted.to_string().len(), 26);
//! ```

use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Fills `dest` with random bytes.
///
/// Uses `/dev/urandom` on Unix-like targets and the documented software
/// fallback everywhere else.
fn fill_random(dest: &mut [u8]) {
    #[cfg(unix)]
    {
        if fill_from_urandom(dest) {
            return;
        }
    }
    fill_from_fallback(dest);
}

/// Reads `/dev/urandom` into `dest`; returns `false` when that fails.
#[cfg(unix)]
fn fill_from_urandom(dest: &mut [u8]) -> bool {
    use std::io::Read;

    match std::fs::File::open("/dev/urandom") {
        Ok(mut file) => file.read_exact(dest).is_ok(),
        Err(_) => false,
    }
}

/// Per-process seed of the fallback PRNG.
static FALLBACK_SEED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

/// Advances the fallback PRNG; concurrent threads never observe the same
/// counter value.
static FALLBACK_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Seeds the fallback PRNG once per process from process-unique values.
fn fallback_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};

    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos() as u64)
        .unwrap_or(0);
    hasher.write_u64(nanos);
    hasher.write_u32(std::process::id());
    hasher.finish()
}

/// Fills `dest` with splitmix64 output seeded by [`fallback_seed`].
///
/// This is a uniqueness-oriented fallback, not a cryptographic generator.
fn fill_from_fallback(dest: &mut [u8]) {
    let seed = *FALLBACK_SEED.get_or_init(fallback_seed);
    for chunk in dest.chunks_mut(8) {
        let counter = FALLBACK_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut state = seed.wrapping_add(counter.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        state = (state ^ (state >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        state = (state ^ (state >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        state ^= state >> 31;
        let bytes = state.to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
}

/// Current Unix time in milliseconds; `0` before the epoch.
fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_millis() as u64)
        .unwrap_or(0)
}

/// Last `(millis, rand_a)` pair emitted by [`Uuid::v7`]; `None` until the
/// first call. Guards monotonic ordering within and across milliseconds,
/// including the epoch edge case where the clock reads `0`.
static V7_STATE: std::sync::Mutex<Option<(u64, u16)>> = std::sync::Mutex::new(None);

/// A 128-bit identifier (RFC 9562) stored raw as 16 bytes.
///
/// Order derives from the byte representation, which makes version 7 IDs
/// lexicographically sortable by creation time.
///
/// ```
/// use akku::id::Uuid;
///
/// let generated = Uuid::v4();
/// assert_eq!(generated.version(), 4);
/// assert_ne!(generated, Uuid::NIL);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uuid([u8; 16]);

impl Uuid {
    /// The all-zero UUID (`00000000-0000-0000-0000-000000000000`).
    pub const NIL: Uuid = Uuid([0u8; 16]);

    /// The all-ones UUID (`ffffffff-ffff-ffff-ffff-ffffffffffff`).
    pub const MAX: Uuid = Uuid([0xFF; 16]);

    /// Generates a random version 4 UUID with RFC 4122 variant bits set.
    ///
    /// ```
    /// use akku::id::Uuid;
    ///
    /// let id = Uuid::v4();
    /// assert_eq!(id.version(), 4);
    /// let bytes = id.as_bytes();
    /// assert_eq!(bytes[8] & 0xC0, 0x80, "variant bits encode RFC 4122");
    /// ```
    pub fn v4() -> Uuid {
        let mut bytes = [0u8; 16];
        fill_random(&mut bytes);
        bytes[6] = (bytes[6] & 0x0F) | 0x40;
        bytes[8] = (bytes[8] & 0x3F) | 0x80;
        Uuid(bytes)
    }

    /// Generates a version 7 UUID (48-bit Unix-millisecond timestamp plus
    /// 74 random bits).
    ///
    /// Consecutive calls are strictly increasing, following RFC 9562
    /// Section 6.2 (method 1): the first identifier of a millisecond draws a
    /// 12-bit counter whose most significant bit is cleared, leaving at least
    /// 2048 monotonic increments in that millisecond. Further calls in the
    /// same millisecond increment the counter; on overflow the timestamp
    /// advances by one millisecond and the counter restarts. If the wall
    /// clock moves backwards or stalls, the last known timestamp is reused
    /// instead of going backwards.
    ///
    /// ```
    /// use akku::id::Uuid;
    ///
    /// let first = Uuid::v7();
    /// let second = Uuid::v7();
    /// assert!(first < second, "consecutive calls stay ordered");
    /// ```
    pub fn v7() -> Uuid {
        let mut rand = [0u8; 10];
        fill_random(&mut rand);
        let now = unix_millis();

        let mut state = V7_STATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (ts, rand_a) = match *state {
            // First call of the process, or a clock that moved forward: seed
            // the counter with the top bit clear, which guarantees at least
            // 2048 further increments inside this millisecond.
            None => (now, rand_a_of(&rand) & 0x07FF),
            Some((prev_ms, prev_seq)) => {
                if now > prev_ms {
                    (now, rand_a_of(&rand) & 0x07FF)
                } else {
                    // Same millisecond, or a wall clock that moved backwards.
                    let next = prev_seq + 1;
                    if next > 0x0FFF {
                        (prev_ms.saturating_add(1), rand_a_of(&rand) & 0x07FF)
                    } else {
                        (prev_ms, next)
                    }
                }
            }
        };
        *state = Some((ts, rand_a));

        Uuid::from_parts(ts, rand_a, rand_b_of(&rand))
    }

    /// Builds a version 7 UUID deterministically from a Unix timestamp in
    /// milliseconds and 10 random bytes.
    ///
    /// The timestamp is masked to 48 bits. Byte layout: `rand[0] & 0x0F` and
    /// `rand[1]` form the 12-bit `rand_a` field (`rand[0]`'s upper nibble is
    /// ignored), `rand[2] & 0x3F` plus `rand[3..10]` form the 62-bit
    /// `rand_b` field.
    ///
    /// ```
    /// use akku::id::Uuid;
    ///
    /// let id = Uuid::v7_at(0x0000_00FF_FFFF_FFFF, [0u8; 10]);
    /// assert_eq!(id.timestamp_millis(), Some(0x0000_00FF_FFFF_FFFF));
    /// assert_eq!(id.version(), 7);
    /// assert_eq!(id.as_bytes()[8] & 0xC0, 0x80);
    /// ```
    pub fn v7_at(unix_millis: u64, rand: [u8; 10]) -> Uuid {
        Uuid::from_parts(unix_millis, rand_a_of(&rand), rand_b_of(&rand))
    }

    /// Assembles a version 7 UUID from already-masked parts.
    fn from_parts(unix_millis: u64, rand_a: u16, rand_b: u64) -> Uuid {
        let mut bytes = [0u8; 16];
        bytes[0..6].copy_from_slice(&unix_millis.to_be_bytes()[2..8]);
        bytes[6] = 0x70 | (rand_a >> 8) as u8;
        bytes[7] = rand_a as u8;
        bytes[8] = 0x80 | (rand_b >> 56) as u8;
        bytes[9..16].copy_from_slice(&rand_b.to_be_bytes()[1..8]);
        Uuid(bytes)
    }

    /// Creates a UUID from its raw 16 bytes.
    pub fn from_bytes(bytes: [u8; 16]) -> Uuid {
        Uuid(bytes)
    }

    /// Borrows the raw 16 bytes.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Consumes the UUID and returns its raw 16 bytes.
    pub fn into_bytes(self) -> [u8; 16] {
        self.0
    }

    /// Creates a UUID from a `u128` (big-endian byte order).
    pub fn from_u128(value: u128) -> Uuid {
        Uuid(value.to_be_bytes())
    }

    /// Returns the UUID as a `u128` (big-endian byte order).
    pub fn as_u128(&self) -> u128 {
        u128::from_be_bytes(self.0)
    }

    /// Returns the RFC 9562 version nibble (`4` or `7` for generated IDs).
    pub fn version(&self) -> u8 {
        self.0[6] >> 4
    }

    /// Returns the Unix-millisecond timestamp of a version 7 UUID.
    pub fn timestamp_millis(&self) -> Option<u64> {
        if self.version() != 7 {
            return None;
        }
        let mut be = [0u8; 8];
        be[2..8].copy_from_slice(&self.0[0..6]);
        Some(u64::from_be_bytes(be))
    }

    /// Parses any common textual UUID form: hyphenated
    /// (`8-4-4-4-12` groups), simple 32 hex digits, braced `{...}` and
    /// `urn:uuid:`-prefixed input — all case-insensitive, surrounding
    /// whitespace ignored.
    ///
    /// Rejects oversized and non-ASCII input before parsing bytes, so
    /// untrusted input of any size costs no allocation.
    pub fn parse(input: &str) -> Result<Uuid, ParseError> {
        let mut text = input.trim();
        const URN_PREFIX: &[u8] = b"urn:uuid:";
        if text.len() >= URN_PREFIX.len()
            && text.as_bytes()[..URN_PREFIX.len()].eq_ignore_ascii_case(URN_PREFIX)
        {
            text = &text[URN_PREFIX.len()..];
        }
        if text.starts_with('{') && text.ends_with('}') && text.len() >= 2 {
            text = &text[1..text.len() - 1];
        }

        // Bound the input before touching its bytes: every accepted form is
        // 32 or 36 ASCII characters. The character count is computed without
        // allocating, so a huge input cannot blow up memory.
        let len = char_len(text);
        let hyphenated = match len {
            36 => true,
            32 => false,
            _ => return Err(ParseError::InvalidLength { len }),
        };
        if !text.is_ascii() {
            return Err(first_non_ascii_char(text));
        }

        let mut bytes = [0u8; 16];
        let mut byte_index = 0usize;
        let mut high_nibble = true;
        for (pos, &byte) in text.as_bytes().iter().enumerate() {
            if hyphenated && matches!(pos, 8 | 13 | 18 | 23) {
                if byte != b'-' {
                    return Err(ParseError::InvalidChar {
                        pos,
                        ch: byte as char,
                    });
                }
                continue;
            }
            let value = hex_value(byte).ok_or(ParseError::InvalidChar {
                pos,
                ch: byte as char,
            })?;
            if high_nibble {
                bytes[byte_index] = value << 4;
            } else {
                bytes[byte_index] |= value;
                byte_index += 1;
            }
            high_nibble = !high_nibble;
        }
        Ok(Uuid(bytes))
    }

    /// Renders the canonical lowercase `8-4-4-4-12` form.
    pub fn to_hyphenated(&self) -> String {
        let mut out = String::with_capacity(36);
        for (index, byte) in self.0.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                out.push('-');
            }
            write_hex_byte(&mut out, *byte);
        }
        out
    }

    /// Renders the 32-digit lowercase form without hyphens.
    pub fn to_simple(&self) -> String {
        let mut out = String::with_capacity(32);
        for byte in self.0 {
            write_hex_byte(&mut out, byte);
        }
        out
    }
}

/// Converts one ASCII hex byte to its nibble value.
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Counts the characters of `text` without allocating.
///
/// ASCII input is its own character count, so the common path is O(1).
fn char_len(text: &str) -> usize {
    if text.is_ascii() {
        text.len()
    } else {
        text.chars().count()
    }
}

/// Builds the [`ParseError::InvalidChar`] for the first non-ASCII character.
fn first_non_ascii_char(text: &str) -> ParseError {
    let byte_pos = text.bytes().position(|byte| !byte.is_ascii()).unwrap_or(0);
    // Every ASCII byte is a character boundary, so the first non-ASCII byte
    // starts a multi-byte character and `byte_pos` is a valid boundary.
    let ch = text[byte_pos..].chars().next().unwrap_or('\u{FFFD}');
    ParseError::InvalidChar {
        pos: text[..byte_pos].chars().count(),
        ch,
    }
}

/// Appends one byte as two lowercase hex digits.
fn write_hex_byte(out: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(HEX[(byte >> 4) as usize] as char);
    out.push(HEX[(byte & 0x0F) as usize] as char);
}

/// Extracts the 12-bit `rand_a` field from the first random bytes.
fn rand_a_of(rand: &[u8; 10]) -> u16 {
    ((rand[0] & 0x0F) as u16) << 8 | rand[1] as u16
}

/// Extracts the 62-bit `rand_b` field from the remaining random bytes.
fn rand_b_of(rand: &[u8; 10]) -> u64 {
    let mut bytes = [0u8; 8];
    bytes[1..8].copy_from_slice(&rand[3..10]);
    ((rand[2] & 0x3F) as u64) << 56 | u64::from_be_bytes(bytes)
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hyphenated())
    }
}

impl fmt::Debug for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for Uuid {
    type Err = ParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Uuid::parse(text)
    }
}

/// Last `(millis, random)` pair emitted by [`Ulid::new`]; makes IDs created
/// in the same millisecond monotonically increasing.
static ULID_STATE: std::sync::Mutex<(u64, u128)> = std::sync::Mutex::new((0, 0));

/// Mask selecting the 80 random bits of a [`Ulid`].
const ULID_RANDOM_MASK: u128 = (1u128 << 80) - 1;

/// Crockford Base32 alphabet used to render [`Ulid`]s.
const CROCKFORD_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A 128-bit, 26-character lexicographically sortable identifier: 48 bits of
/// Unix-millisecond timestamp and 80 bits of randomness, both stored in one
/// `u128`.
///
/// ```
/// use akku::id::Ulid;
///
/// let ulid = Ulid::from_parts(1, 2);
/// assert_eq!(ulid.timestamp_ms(), 1);
/// assert_eq!(ulid.random(), 2);
/// assert_eq!(ulid.to_string().len(), 26);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ulid(u128);

impl Ulid {
    /// Generates an identifier from the current time plus 80 random bits.
    ///
    /// Identifiers created within the same millisecond increment the random
    /// part monotonically (and the last known timestamp is reused if the
    /// clock runs backwards), so consecutive calls are ordered.
    ///
    /// ```
    /// use akku::id::Ulid;
    ///
    /// let first = Ulid::new();
    /// let second = Ulid::new();
    /// assert!(first <= second, "same-millisecond IDs increment");
    /// ```
    pub fn new() -> Ulid {
        let mut rand = [0u8; 10];
        fill_random(&mut rand);
        let mut bytes = [0u8; 16];
        bytes[6..16].copy_from_slice(&rand);
        let fresh_random = u128::from_be_bytes(bytes) & ULID_RANDOM_MASK;

        let mut millis = unix_millis();
        let mut state = ULID_STATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if millis < state.0 {
            millis = state.0;
        }
        let random = if millis == state.0 && state.1 < ULID_RANDOM_MASK {
            state.1 + 1
        } else {
            fresh_random
        };
        *state = (millis, random);

        Ulid((millis as u128) << 80 | random)
    }

    /// Builds a ULID deterministically; the timestamp is masked to 48 bits
    /// and the random part to 80 bits.
    pub fn from_parts(timestamp_ms: u64, random: u128) -> Ulid {
        Ulid(((timestamp_ms & 0xFFFF_FFFF_FFFF) as u128) << 80 | random & ULID_RANDOM_MASK)
    }

    /// Interprets 16 bytes (big-endian) as a ULID value.
    ///
    /// ```
    /// use akku::id::Ulid;
    ///
    /// let ulid = Ulid::from_bytes([0xFF; 16]);
    /// assert_eq!(ulid.timestamp_ms(), u64::MAX >> (64 - 48));
    /// ```
    pub fn from_bytes(bytes: [u8; 16]) -> Ulid {
        Ulid(u128::from_be_bytes(bytes))
    }

    /// Returns the value as 16 big-endian bytes.
    pub fn to_bytes(&self) -> [u8; 16] {
        self.0.to_be_bytes()
    }

    /// Returns the 48-bit Unix-millisecond timestamp part.
    pub fn timestamp_ms(&self) -> u64 {
        (self.0 >> 80) as u64
    }

    /// Returns the 80-bit random part.
    pub fn random(&self) -> u128 {
        self.0 & ULID_RANDOM_MASK
    }

    /// Parses a Crockford Base32 string (1 to 26 characters,
    /// case-insensitive; `I`/`L` read as `1`, `O` as `0`; `U` is rejected).
    ///
    /// Values that exceed 128 bits (for example a string starting with `8`
    /// in the 26th-most-significant digit) are rejected as
    /// [`ParseError::Overflow`].
    ///
    /// The length and the ASCII range are checked before any decoding, so
    /// untrusted input of any size costs no allocation.
    pub fn parse(input: &str) -> Result<Ulid, ParseError> {
        let text = input.trim();
        let len = char_len(text);
        if len == 0 || len > 26 {
            return Err(ParseError::InvalidLength { len });
        }
        if !text.is_ascii() {
            return Err(first_non_ascii_char(text));
        }
        let mut value: u128 = 0;
        for (pos, &byte) in text.as_bytes().iter().enumerate() {
            let digit = crockford_digit(byte).ok_or(ParseError::InvalidChar {
                pos,
                ch: byte as char,
            })?;
            value = value
                .checked_mul(32)
                .and_then(|shifted| shifted.checked_add(digit as u128))
                .ok_or(ParseError::Overflow)?;
        }
        Ok(Ulid(value))
    }

    /// Renders exactly 26 Crockford Base32 characters.
    fn encode(&self) -> String {
        let mut out = String::with_capacity(26);
        let mut value = self.0;
        let mut digits = [0u8; 26];
        for index in (0..26).rev() {
            digits[index] = CROCKFORD_ALPHABET[(value % 32) as usize];
            value /= 32;
        }
        for digit in digits {
            out.push(digit as char);
        }
        out
    }
}

impl Default for Ulid {
    /// Shorthand for [`Ulid::new`].
    fn default() -> Self {
        Self::new()
    }
}

/// Maps an ASCII byte to its Crockford Base32 digit value (`None` if the
/// byte is not valid — including `U`).
fn crockford_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0' | b'o' | b'O' => Some(0),
        b'1' | b'i' | b'I' | b'l' | b'L' => Some(1),
        b'2' => Some(2),
        b'3' => Some(3),
        b'4' => Some(4),
        b'5' => Some(5),
        b'6' => Some(6),
        b'7' => Some(7),
        b'8' => Some(8),
        b'9' => Some(9),
        b'a' | b'A' => Some(10),
        b'b' | b'B' => Some(11),
        b'c' | b'C' => Some(12),
        b'd' | b'D' => Some(13),
        b'e' | b'E' => Some(14),
        b'f' | b'F' => Some(15),
        b'g' | b'G' => Some(16),
        b'h' | b'H' => Some(17),
        b'j' | b'J' => Some(18),
        b'k' | b'K' => Some(19),
        b'm' | b'M' => Some(20),
        b'n' | b'N' => Some(21),
        b'p' | b'P' => Some(22),
        b'q' | b'Q' => Some(23),
        b'r' | b'R' => Some(24),
        b's' | b'S' => Some(25),
        b't' | b'T' => Some(26),
        b'v' | b'V' => Some(27),
        b'w' | b'W' => Some(28),
        b'x' | b'X' => Some(29),
        b'y' | b'Y' => Some(30),
        b'z' | b'Z' => Some(31),
        _ => None,
    }
}

impl fmt::Display for Ulid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

impl fmt::Debug for Ulid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for Ulid {
    type Err = ParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Ulid::parse(text)
    }
}

/// The 64-symbol, URL-safe default NanoID alphabet.
pub const DEFAULT_ALPHABET: &[u8] =
    b"_-0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// Generates the standard 21-character NanoID.
///
/// ```
/// use akku::id::{nanoid, DEFAULT_ALPHABET};
///
/// let id = nanoid();
/// assert_eq!(id.len(), 21);
/// assert!(id.chars().all(|c| DEFAULT_ALPHABET.contains(&(c as u8))));
/// ```
pub fn nanoid() -> String {
    nanoid_len(21)
}

/// Generates a NanoID of an arbitrary length using
/// [`DEFAULT_ALPHABET`].
///
/// ```
/// use akku::id::{nanoid_len, DEFAULT_ALPHABET};
///
/// let id = nanoid_len(10);
/// assert_eq!(id.len(), 10);
/// assert!(id.chars().all(|c| DEFAULT_ALPHABET.contains(&(c as u8))));
/// ```
pub fn nanoid_len(length: usize) -> String {
    random_string(length, DEFAULT_ALPHABET.len(), |index| {
        DEFAULT_ALPHABET[index] as char
    })
}

/// Generates a NanoID over a caller-provided alphabet.
///
/// The alphabet must hold between 1 and 256 symbols; sampling uses
/// rejection sampling, so every symbol is equally likely (no modulo bias).
///
/// ```
/// use akku::id::nanoid_custom;
///
/// let id = nanoid_custom(8, &['a', 'b']).unwrap();
/// assert_eq!(id.len(), 8);
/// assert!(id.chars().all(|c| c == 'a' || c == 'b'));
/// ```
pub fn nanoid_custom(length: usize, alphabet: &[char]) -> Result<String, IdError> {
    if alphabet.is_empty() {
        return Err(IdError::EmptyAlphabet);
    }
    if alphabet.len() > 256 {
        return Err(IdError::AlphabetTooLarge {
            len: alphabet.len(),
        });
    }
    Ok(random_string(length, alphabet.len(), |index| {
        alphabet[index]
    }))
}

/// Rejection-sampled, unbiased index picking over `alphabet_len` symbols.
///
/// One random byte covers up to 256 symbols: the byte is masked to the next
/// power of two minus one, and draws outside `alphabet_len` are redrawn.
fn random_string(length: usize, alphabet_len: usize, symbol: impl Fn(usize) -> char) -> String {
    let mask = alphabet_len.next_power_of_two().saturating_sub(1) as u8;
    let mut out = String::with_capacity(length);
    let mut pool = [0u8; 64];
    let mut used = pool.len();
    let mut generated = 0;
    while generated < length {
        if used == pool.len() {
            fill_random(&mut pool);
            used = 0;
        }
        let index = (pool[used] & mask) as usize;
        used += 1;
        if index < alphabet_len {
            out.push(symbol(index));
            generated += 1;
        }
    }
    out
}

/// Error returned by the NanoID constructors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdError {
    /// The alphabet has no symbols.
    EmptyAlphabet,
    /// The alphabet has more symbols than one random byte can address.
    AlphabetTooLarge {
        /// The offending alphabet length.
        len: usize,
    },
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdError::EmptyAlphabet => f.write_str("ID alphabet must contain at least one symbol"),
            IdError::AlphabetTooLarge { len } => {
                write!(
                    f,
                    "ID alphabet has {len} symbols; at most 256 are supported"
                )
            }
        }
    }
}

impl std::error::Error for IdError {}

/// Error returned when parsing a [`Uuid`] or [`Ulid`] fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// The input has the wrong number of characters.
    InvalidLength {
        /// The observed character count.
        len: usize,
    },
    /// The input contains a character that is not valid in this position.
    InvalidChar {
        /// Character position (0-based).
        pos: usize,
        /// The offending character.
        ch: char,
    },
    /// The decoded value does not fit into 128 bits.
    Overflow,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::InvalidLength { len } => {
                write!(
                    f,
                    "identifier has {len} characters, expected 32, 36 or at most 26"
                )
            }
            ParseError::InvalidChar { pos, ch } => {
                write!(f, "invalid character {ch:?} at position {pos}")
            }
            ParseError::Overflow => f.write_str("identifier value overflows 128 bits"),
        }
    }
}

impl std::error::Error for ParseError {}
