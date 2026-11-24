//! Internal RFC 3174 SHA-1 and masking-key entropy helpers for RFC 6455 WebSockets.
//!
//! SHA-1 is implemented here **solely** to compute the RFC 6455 §1.3
//! `Sec-WebSocket-Accept` handshake token (`base64(SHA-1(key || GUID))`).
//! It is never exposed publicly and must not be used for cryptographic security.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;

/// Computes the 20-byte RFC 3174 SHA-1 digest of `input`.
pub(crate) fn digest(input: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x6745_2301;
    let mut h1: u32 = 0xEFCD_AB89;
    let mut h2: u32 = 0x98BA_DCFE;
    let mut h3: u32 = 0x1032_5476;
    let mut h4: u32 = 0xC3D2_E1F0;

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((input.len() + 9).div_ceil(64) * 64);
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut w = [0u32; 80];
    for block in padded.chunks_exact(64) {
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let offset = i * 4;
            *word = u32::from_be_bytes([
                block[offset],
                block[offset + 1],
                block[offset + 2],
                block[offset + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;

        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC_u32),
                _ => (b ^ c ^ d, 0xCA62_C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

/// Fills `dest` with unpredictable bytes for `Sec-WebSocket-Key` nonces and
/// RFC 6455 §5.3 frame masking keys.
///
/// Masking keys only need to be unpredictable to the peer (RFC 6455 §10.3); they
/// are not long-term secrets. Bytes come from a process-wide [`KeyStream`]: a
/// SipHash-keyed counter-mode stream (std's `RandomState` hasher) that is seeded
/// **once** per process. No file is opened per call; the only per-call syscall is
/// a `getpid` used to detect `fork()` and reseed the child (see [`KeyStream`]).
pub(crate) fn fill_random(dest: &mut [u8]) {
    static STREAM: OnceLock<KeyStream> = OnceLock::new();
    STREAM
        .get_or_init(|| {
            #[cfg(test)]
            SHARED_STREAM_SEEDINGS.fetch_add(1, Ordering::Relaxed);
            KeyStream::from_entropy()
        })
        .fill(dest);
}

/// Number of times the shared stream behind [`fill_random`] was seeded (test observability).
#[cfg(test)]
static SHARED_STREAM_SEEDINGS: AtomicU64 = AtomicU64::new(0);

/// SipHash-keyed counter-mode byte stream used for masking keys and nonces.
///
/// Every 8-byte block is `SipHash_k(seed || counter)` where `k` is the per-process
/// random key of a [`RandomState`] and `seed` is 128 bits of entropy gathered once.
/// The atomic counter guarantees distinct blocks across calls, threads, and
/// connections.
///
/// `fork()` duplicates seed, keys, and counter, so on targets with processes
/// every block additionally mixes the current pid and a per-pid salt: when
/// [`KeyStream::fill`] observes a pid different from the one it was seeded in,
/// it draws a fresh salt from OS entropy, so parent and child (and successive
/// children reusing a pid) diverge.
///
/// [`RandomState`]: std::collections::hash_map::RandomState
#[derive(Debug)]
pub(crate) struct KeyStream {
    keys: std::collections::hash_map::RandomState,
    seed: [u64; 2],
    counter: AtomicU64,
    /// Pid the current `salt` belongs to.
    pid: AtomicU32,
    /// Extra entropy redrawn whenever the pid changes (after `fork()`).
    salt: AtomicU64,
    /// Number of pid-change reseeds (test observability).
    #[cfg(test)]
    reseeds: AtomicU64,
}

impl KeyStream {
    /// Seeds a new stream from OS entropy, gathered exactly once.
    ///
    /// On Unix this reads 16 bytes from `/dev/urandom`. When that is unavailable
    /// (e.g. `wasm32`, sandboxed processes), the seed is instead derived from a
    /// fresh `RandomState` (itself seeded from OS randomness by std where the
    /// platform provides it), the wall clock, the process id, and the address of a
    /// stack/heap location. This fallback is never a fixed constant, but on
    /// platforms where std has no OS randomness it is only as unpredictable as the
    /// clock and address-space layout; that is acceptable for RFC 6455 masking,
    /// which only has to defeat cache-poisoning by a non-colluding peer.
    pub(crate) fn from_entropy() -> Self {
        Self::from_seed([entropy_u64(), entropy_u64()])
    }

    /// Creates a stream from an explicit seed (keys still come from `RandomState`).
    pub(crate) fn from_seed(seed: [u64; 2]) -> Self {
        Self {
            keys: std::collections::hash_map::RandomState::new(),
            seed,
            counter: AtomicU64::new(0),
            pid: AtomicU32::new(current_pid()),
            salt: AtomicU64::new(0),
            #[cfg(test)]
            reseeds: AtomicU64::new(0),
        }
    }

    /// Reseeds the per-pid salt if `pid` differs from the pid last observed.
    ///
    /// Lock-free (no mutex that a `fork()` could leave held): concurrent
    /// reseeds in the child are benign because each one draws fresh entropy.
    fn observe_pid(&self, pid: u32) {
        if self.pid.load(Ordering::Acquire) != pid {
            self.salt.store(entropy_u64(), Ordering::Release);
            self.pid.store(pid, Ordering::Release);
            #[cfg(test)]
            self.reseeds.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Returns the next 64-bit block of the stream.
    pub(crate) fn next_u64(&self) -> u64 {
        use std::hash::{BuildHasher, Hasher};

        let counter = self.counter.fetch_add(1, Ordering::Relaxed);
        let mut hasher = self.keys.build_hasher();
        hasher.write_u64(self.seed[0]);
        hasher.write_u64(self.seed[1]);
        hasher.write_u64(self.salt.load(Ordering::Acquire));
        hasher.write_u32(self.pid.load(Ordering::Acquire));
        hasher.write_u64(counter);
        hasher.finish()
    }

    /// Fills `dest` with stream bytes, reseeding first if the process forked.
    pub(crate) fn fill(&self, dest: &mut [u8]) {
        self.fill_as_pid(dest, current_pid());
    }

    /// [`Self::fill`] with an explicit observed pid (lets tests simulate `fork()`).
    fn fill_as_pid(&self, dest: &mut [u8], pid: u32) {
        self.observe_pid(pid);
        for chunk in dest.chunks_mut(8) {
            let bytes = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }
}

/// Returns the current process id, or `0` where the target has no processes.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn current_pid() -> u32 {
    std::process::id()
}

/// `wasm32-unknown-unknown` has no processes (`std::process::id` panics there).
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
const fn current_pid() -> u32 {
    0
}

/// Returns 64 bits of OS entropy (`/dev/urandom` on Unix) mixed with [`fallback_entropy`].
fn entropy_u64() -> u64 {
    #[allow(unused_mut)]
    let mut value = fallback_entropy();
    #[cfg(unix)]
    {
        let mut os = [0u8; 8];
        if read_urandom(&mut os) {
            // Mix (XOR) rather than replace, so either source alone suffices.
            value ^= u64::from_le_bytes(os);
        }
    }
    value
}

#[cfg(unix)]
fn read_urandom(dest: &mut [u8]) -> bool {
    use std::io::Read;

    match std::fs::File::open("/dev/urandom") {
        Ok(mut file) => file.read_exact(dest).is_ok(),
        Err(_) => false,
    }
}

/// Best-effort entropy without OS randomness: `RandomState` + time + pid + ASLR + counter.
fn fallback_entropy() -> u64 {
    use std::hash::{BuildHasher, Hasher};

    static CALLS: AtomicU64 = AtomicU64::new(0);

    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    // `SystemTime::now` and `std::process::id` panic on `wasm32-unknown-unknown`.
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        hasher.write_u128(nanos);
    }
    hasher.write_u32(current_pid());
    let stack_marker = 0u8;
    hasher.write_usize(std::ptr::addr_of!(stack_marker) as usize);
    let heap_marker = Box::new(0u8);
    hasher.write_usize(std::ptr::addr_of!(*heap_marker) as usize);
    hasher.write_u64(CALLS.fetch_add(1, Ordering::Relaxed));
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_random_gathers_entropy_once_per_process() {
        let mut key = [0u8; 4];
        fill_random(&mut key);
        for _ in 0..1000 {
            fill_random(&mut key);
        }
        // Seeded exactly once: no further `/dev/urandom` open per frame.
        assert_eq!(SHARED_STREAM_SEEDINGS.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn key_stream_blocks_are_distinct() {
        let stream = KeyStream::from_seed([0, 0]);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..10_000 {
            assert!(seen.insert(stream.next_u64()));
        }
    }

    #[test]
    fn key_streams_with_same_seed_differ_by_hash_keys() {
        let a = KeyStream::from_seed([1, 2]);
        let b = KeyStream::from_seed([1, 2]);
        let xs: Vec<u64> = (0..4).map(|_| a.next_u64()).collect();
        let ys: Vec<u64> = (0..4).map(|_| b.next_u64()).collect();
        assert_ne!(xs, ys);
    }

    #[test]
    fn entropy_seeds_differ() {
        let a = KeyStream::from_entropy();
        let b = KeyStream::from_entropy();
        assert_ne!(a.seed, b.seed);
        assert_ne!(fallback_entropy(), fallback_entropy());
    }

    #[test]
    fn pid_change_reseeds_salt_once() {
        let stream = KeyStream::from_seed([3, 4]);
        let own = current_pid();
        let mut buf = [0u8; 8];
        stream.fill_as_pid(&mut buf, own);
        assert_eq!(stream.reseeds.load(Ordering::Relaxed), 0);
        let salt_before = stream.salt.load(Ordering::Relaxed);

        // Simulated fork: a different pid triggers exactly one reseed.
        let child = own.wrapping_add(1);
        stream.fill_as_pid(&mut buf, child);
        assert_eq!(stream.reseeds.load(Ordering::Relaxed), 1);
        assert_eq!(stream.pid.load(Ordering::Relaxed), child);
        assert_ne!(stream.salt.load(Ordering::Relaxed), salt_before);
        stream.fill_as_pid(&mut buf, child);
        assert_eq!(stream.reseeds.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn forked_copies_diverge_at_same_counter() {
        // A "forked" child shares keys, seed, and counter with the parent.
        let parent = KeyStream::from_seed([5, 6]);
        let child = KeyStream {
            keys: parent.keys.clone(),
            seed: parent.seed,
            counter: AtomicU64::new(0),
            pid: AtomicU32::new(parent.pid.load(Ordering::Relaxed)),
            salt: AtomicU64::new(parent.salt.load(Ordering::Relaxed)),
            reseeds: AtomicU64::new(0),
        };
        let own = current_pid();
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        parent.fill_as_pid(&mut a, own);
        child.fill_as_pid(&mut b, own.wrapping_add(1));
        assert_ne!(a, b);
    }

    #[test]
    fn fill_handles_partial_blocks() {
        let stream = KeyStream::from_seed([7, 7]);
        let mut buf = [0u8; 13];
        stream.fill(&mut buf);
        assert!(buf.iter().any(|&b| b != 0));
    }
}
