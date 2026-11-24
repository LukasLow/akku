//! CRC-32 (IEEE 802.3, reflected polynomial) and Adler-32 checksums.
//!
//! Pure safe Rust, table-driven, usable standalone or incrementally. These
//! power the gzip and zip trailer/record verification.
//!
//! ```
//! use akku::archive::checksum::{adler32, crc32};
//!
//! assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
//! assert_eq!(adler32(b"123456789"), 0x091E_01DE);
//! assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
//! ```

/// Build the reflected CRC-32 lookup table (polynomial `0xEDB8_83EF`) at
/// compile time.
const fn build_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

/// The CRC-32 lookup table, built by [`build_crc32_table`].
static CRC32_TABLE: [u32; 256] = build_crc32_table();

/// Incremental CRC-32 (IEEE) computation.
///
/// ```
/// use akku::archive::checksum::Crc32;
///
/// let mut crc = Crc32::new();
/// crc.update(b"123");
/// crc.update(b"456789");
/// assert_eq!(crc.value(), 0xCBF4_3926);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Crc32 {
    /// The running register (pre-inversion).
    value: u32,
}

impl Crc32 {
    /// Start a fresh checksum (equivalent to the checksum of no bytes).
    pub const fn new() -> Self {
        Self { value: 0xFFFF_FFFF }
    }

    /// Fold `data` into the checksum.
    pub fn update(&mut self, data: &[u8]) {
        let mut v = self.value;
        for &b in data {
            v = CRC32_TABLE[((v ^ b as u32) & 0xFF) as usize] ^ (v >> 8);
        }
        self.value = v;
    }

    /// The finished CRC-32 value.
    pub const fn value(&self) -> u32 {
        !self.value
    }

    /// Reset to the initial state.
    pub fn reset(&mut self) {
        self.value = 0xFFFF_FFFF;
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

/// One-shot CRC-32 (IEEE) of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update(data);
    c.value()
}

/// The Adler-32 modulus (largest prime below 65536).
const ADLER_MOD: u32 = 65_521;

/// Largest block processed before the deferred modulo is applied; chosen so
/// 32-bit arithmetic can never overflow (as in zlib).
const ADLER_NMAX: usize = 5552;

/// Incremental Adler-32 computation (RFC 1950).
///
/// ```
/// use akku::archive::checksum::Adler32;
///
/// let mut adler = Adler32::new();
/// adler.update(b"Wiki");
/// adler.update(b"pedia");
/// assert_eq!(adler.value(), 0x11E6_0398);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Adler32 {
    /// Sum of all bytes plus 1.
    s1: u32,
    /// Sum of all `s1` states.
    s2: u32,
}

impl Adler32 {
    /// Start a fresh checksum (value 1, matching the RFC's initial state).
    pub const fn new() -> Self {
        Self { s1: 1, s2: 0 }
    }

    /// Fold `data` into the checksum.
    pub fn update(&mut self, data: &[u8]) {
        for chunk in data.chunks(ADLER_NMAX) {
            for &b in chunk {
                self.s1 += b as u32;
                self.s2 += self.s1;
            }
            self.s1 %= ADLER_MOD;
            self.s2 %= ADLER_MOD;
        }
    }

    /// The finished Adler-32 value.
    pub const fn value(&self) -> u32 {
        (self.s2 << 16) | self.s1
    }

    /// Reset to the initial state.
    pub fn reset(&mut self) {
        self.s1 = 1;
        self.s2 = 0;
    }
}

impl Default for Adler32 {
    fn default() -> Self {
        Self::new()
    }
}

/// One-shot Adler-32 of `data`.
pub fn adler32(data: &[u8]) -> u32 {
    let mut a = Adler32::new();
    a.update(data);
    a.value()
}
