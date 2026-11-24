//! Integration tests for `akku::archive`.
//!
//! Covers RFC 1951/1952 golden vectors, roundtrips across sizes and
//! compressibility, zip-bomb output caps, zip-slip path traversal rejection,
//! USTAR/PAX long names, and (when the tools exist in the container)
//! interop with the system `gzip` and `tar`.

use std::io::{Read, Write};
use std::process::Command;

use akku::archive::checksum::{adler32, crc32, Adler32, Crc32};
use akku::archive::deflate::{
    self, CompressionLevel, DeflateEncoder, DeflateError, InflateDecoder, InflateStream,
};
use akku::archive::error::ExtractError;
use akku::archive::gzip::{self, GzipDecoder, GzipError, GzipHeader, GzipOptions};
use akku::archive::tar::{EntryType, TarError, TarReader, TarWriter};
use akku::archive::zip::{Method, ZipError, ZipReader, ZipWriter};
use akku::archive::ExtractOptions;
use akku::test::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Deterministic pseudo-random bytes (incompressible-ish) from an LCG.
fn lcg_bytes(len: usize) -> Vec<u8> {
    let mut state = 0x1234_5678u64;
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u8
        })
        .collect()
}

/// Highly compressible repetitive text.
fn lorem(rep: usize) -> Vec<u8> {
    b"Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(rep)
}

/// LSB-first bit builder for hand-crafted DEFLATE vectors.
struct BitBuilder {
    bytes: Vec<u8>,
    bit: u32,
}

impl BitBuilder {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            bit: 0,
        }
    }

    /// Write the low `n` bits of `value`, least-significant bit first.
    fn put(&mut self, value: u32, n: u32) {
        for i in 0..n {
            let b = (value >> i) & 1;
            if self.bit == 0 {
                self.bytes.push(0);
            }
            if b == 1 {
                *self.bytes.last_mut().unwrap() |= 1 << self.bit;
            }
            self.bit = (self.bit + 1) % 8;
        }
    }

    /// Write a Huffman code in natural (MSB-first) order.
    fn put_code(&mut self, code: u32, len: u32) {
        for i in (0..len).rev() {
            self.put((code >> i) & 1, 1);
        }
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Whether an external tool exists (used to skip interop tests gracefully).
fn has_tool(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Run a tool capturing stdout; `None` on failure or absence.
fn run_tool(cmd: &str, args: &[&str], cwd: Option<&std::path::Path>) -> Option<Vec<u8>> {
    let mut c = Command::new(cmd);
    c.args(args);
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }
    c.stdout(std::process::Stdio::piped());
    let out = c.output().ok()?;
    if out.status.success() {
        Some(out.stdout)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// DEFLATE
// ---------------------------------------------------------------------------

#[test]
fn crc32_known_vectors() {
    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    let mut c = Crc32::new();
    c.update(b"123");
    c.update(b"456");
    c.update(b"789");
    assert_eq!(c.value(), 0xCBF4_3926);
    c.reset();
    assert_eq!(c.value(), 0);
}

#[test]
fn adler32_known_vectors() {
    assert_eq!(adler32(b""), 1);
    assert_eq!(adler32(b"123456789"), 0x091E_01DE);
    assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    let mut a = Adler32::new();
    a.update(b"Wiki");
    a.update(b"pedia");
    assert_eq!(a.value(), 0x11E6_0398);
}

#[test]
fn deflate_roundtrip_all_levels_and_sizes() {
    let cases: Vec<Vec<u8>> = vec![
        Vec::new(),
        b"x".to_vec(),
        b"abcabcabcabc".to_vec(),
        lcg_bytes(32),
        lcg_bytes(511),
        lcg_bytes(512),
        lcg_bytes(513),
        lcg_bytes(65_536),
        lcg_bytes(65_537),
        vec![0u8; 100_000],
        lorem(2_000),
    ];
    for data in &cases {
        for level in [
            CompressionLevel::Stored,
            CompressionLevel::Fast,
            CompressionLevel::Normal,
            CompressionLevel::Best,
        ] {
            let packed = deflate::deflate(data, level);
            let cap = data.len() as u64 * 2 + 256;
            let unpacked = deflate::inflate(&packed, cap).unwrap();
            assert_eq!(unpacked, *data, "level {level:?}, len {}", data.len());
        }
    }
}

#[test]
fn deflate_golden_streams() {
    // Empty streams: fixed-code final block is exactly [0x03, 0x00];
    // empty stored block is exactly [0x01, 0x00, 0x00, 0xFF, 0xFF].
    assert_eq!(deflate::deflate(b"", CompressionLevel::Fast), [0x03, 0x00]);
    assert_eq!(
        deflate::deflate(b"", CompressionLevel::Normal),
        [0x03, 0x00]
    );
    assert_eq!(deflate::deflate(b"", CompressionLevel::Best), [0x03, 0x00]);
    assert_eq!(
        deflate::deflate(b"", CompressionLevel::Stored),
        [0x01, 0x00, 0x00, 0xFF, 0xFF]
    );
    // Empty input is not a valid compressed stream.
    assert!(matches!(
        deflate::inflate(b"", 1024),
        Err(DeflateError::TruncatedStream)
    ));
    // Hand-built fixed-Huffman block: literal 'A', end-of-block.
    let mut b = BitBuilder::new();
    b.put(1, 1); // BFINAL
    b.put(1, 2); // BTYPE 01
    b.put_code(0x30 + 65, 8); // literal 'A'
    b.put_code(0, 7); // end-of-block (code 0000000)
    let stream = b.into_bytes();
    assert_eq!(deflate::inflate(&stream, 16).unwrap(), b"A");
}

#[test]
fn deflate_zip_bomb_output_limit() {
    let bomb = deflate::deflate(&vec![0u8; 1 << 20], CompressionLevel::Best);
    assert!(
        bomb.len() < 4_096,
        "1 MiB of zeros should compress to ~1 KiB, got {}",
        bomb.len()
    );
    assert!(matches!(
        deflate::inflate(&bomb, 64 * 1024),
        Err(DeflateError::OutputLimitExceeded)
    ));
    assert_eq!(deflate::inflate(&bomb, 2 << 20).unwrap().len(), 1 << 20);
}

#[test]
fn deflate_malformed_streams_rejected() {
    // Reserved block type 3.
    let mut b = BitBuilder::new();
    b.put(1, 1);
    b.put(3, 2);
    let s = b.into_bytes();
    assert!(matches!(
        deflate::inflate(&s, 64),
        Err(DeflateError::InvalidBlockType(3))
    ));
    // Stored block with LEN != !NLEN.
    let s2 = [0x01u8, 0x05, 0x00, 0x00, 0x00];
    assert!(matches!(
        deflate::inflate(&s2, 64),
        Err(DeflateError::InvalidStoredBlockLen)
    ));
    // Fixed block whose match distance exceeds the output so far.
    let mut b = BitBuilder::new();
    b.put(1, 1);
    b.put(1, 2);
    b.put_code(0x30 + 65, 8); // literal 'A' (1 byte of output)
    b.put_code(1, 7); // length symbol 257 => len 3
    b.put_code(3, 5); // distance symbol 3 => base 4 > 1
    b.put_code(0, 7); // end-of-block
    let s3 = b.into_bytes();
    assert!(matches!(
        deflate::inflate(&s3, 64),
        Err(DeflateError::DistanceTooFarBack)
    ));
    // Over-subscribed code-length table in a dynamic header.
    let mut b = BitBuilder::new();
    b.put(1, 1);
    b.put(2, 2); // BTYPE 10 (dynamic)
    b.put(0, 5); // HLIT = 257
    b.put(0, 5); // HDIST = 1
    b.put(0, 4); // HCLEN = 4
    for _ in 0..4 {
        b.put(1, 3); // four 1-bit code lengths: over-subscribed
    }
    let s4 = b.into_bytes();
    assert!(matches!(
        deflate::inflate(&s4, 64),
        Err(DeflateError::InvalidHuffmanCode)
    ));
}

#[test]
fn deflate_truncated_stream_rejected() {
    let data = lcg_bytes(10_000);
    let packed = deflate::deflate(&data, CompressionLevel::Normal);
    for cut in [packed.len() / 4, packed.len() / 2, 3 * packed.len() / 4] {
        let res = deflate::inflate(&packed[..cut], 1 << 20);
        assert!(
            matches!(
                res,
                Err(DeflateError::TruncatedStream) | Err(DeflateError::InvalidHuffmanCode)
            ),
            "cut at {cut}"
        );
    }
}

#[test]
fn deflate_streaming_encoder_chunking_invariance() {
    let data = lcg_bytes(150_000); // spans multiple blocks
    for level in [
        CompressionLevel::Stored,
        CompressionLevel::Fast,
        CompressionLevel::Normal,
        CompressionLevel::Best,
    ] {
        let one_shot = deflate::deflate(&data, level);
        for chunk in [1usize, 7, 1_000, 70_000] {
            let mut enc = DeflateEncoder::new(Vec::new(), level);
            for piece in data.chunks(chunk) {
                enc.write_all(piece).unwrap();
            }
            let streamed = enc.finish().unwrap();
            assert_eq!(streamed, one_shot, "chunk {chunk}, level {level:?}");
        }
    }
}

#[test]
fn deflate_streaming_decoder_roundtrip_and_limit() {
    let data = lcg_bytes(80_000);
    let packed = deflate::deflate(&data, CompressionLevel::Normal);
    let mut dec = InflateDecoder::new(&packed[..], 1 << 20);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).unwrap();
    dec.finish().unwrap();
    assert_eq!(out, data);
    // Output cap trips mid-stream.
    let mut dec = InflateDecoder::new(&packed[..], 1_000);
    let res = dec.read_to_end(&mut Vec::new());
    assert!(res.is_err());
    // Push/pull core across tiny chunk boundaries.
    let mut stream = InflateStream::new(1 << 20);
    let mut out = Vec::new();
    let mut fed = 0usize;
    let mut rounds = 0usize;
    while !stream.is_finished() {
        rounds += 1;
        assert!(rounds < 20_000, "push/pull made no progress");
        let end = (fed + 13).min(packed.len());
        let consumed = stream.push(&packed[fed..end]).unwrap();
        fed += consumed;
        let mut buf = [0u8; 37];
        loop {
            let n = stream.pull(&mut buf);
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
    }
    assert_eq!(out, data);
}

#[test]
fn deflate_fuzz_never_panics() {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut rnd = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as u32
    };
    let sample = deflate::deflate(&lorem(20), CompressionLevel::Best);
    for i in 0..2000u32 {
        let len = (rnd() % 64) as usize;
        let mut buf: Vec<u8> = (0..len).map(|_| rnd() as u8).collect();
        if i % 2 == 0 && !sample.is_empty() {
            // Mutations of a valid stream: truncation + random tail.
            let cut = (rnd() as usize) % sample.len();
            buf = sample[..cut].to_vec();
            buf.push(rnd() as u8);
        }
        let _ = deflate::inflate(&buf, 4096); // must never panic
    }
}

// ---------------------------------------------------------------------------
// Gzip
// ---------------------------------------------------------------------------

#[test]
fn gzip_roundtrip_canonical_sizes() {
    let empty = gzip::compress(b"");
    assert_eq!(empty.len(), 20);
    assert_eq!(&empty[0..3], &[0x1F, 0x8B, 0x08]);
    assert_eq!(gzip::decompress(&empty).unwrap(), b"");
    let cases: Vec<Vec<u8>> = vec![
        b"a".to_vec(),
        lcg_bytes(65_536),
        vec![0u8; 65_536],
        lorem(500),
    ];
    for data in &cases {
        for level in [
            CompressionLevel::Stored,
            CompressionLevel::Fast,
            CompressionLevel::Normal,
            CompressionLevel::Best,
        ] {
            let gz = gzip::compress_with(data, &GzipOptions::new().level(level));
            assert_eq!(gzip::decompress(&gz).unwrap(), *data);
        }
    }
    // 1 MiB of zeros under the default cap.
    let big = vec![0u8; 1 << 20];
    let gz = gzip::compress(&big);
    assert_eq!(gzip::decompress(&gz).unwrap().len(), 1 << 20);
}

#[test]
fn gzip_header_fields_roundtrip() {
    let opts = GzipOptions::new()
        .mtime(1_700_000_000)
        .filename("data.txt")
        .comment("hello")
        .extra(vec![1, 2, 3, 4])
        .os(7);
    let gz = gzip::compress_with(b"payload", &opts);
    let mut slice = &gz[..];
    let h = GzipHeader::read_from(&mut slice).unwrap();
    assert_eq!(h.mtime, 1_700_000_000);
    assert_eq!(h.filename.as_deref(), Some("data.txt"));
    assert_eq!(h.comment.as_deref(), Some("hello"));
    assert_eq!(h.extra.as_deref(), Some(&[1u8, 2, 3, 4][..]));
    assert_eq!(h.os, 7);
    // Reserved flag bits are rejected.
    let mut bad = gzip::compress(b"x");
    bad[3] |= 0xE0;
    assert!(matches!(
        gzip::decompress(&bad),
        Err(GzipError::ReservedFlagSet(_))
    ));
    // FHCRC (header CRC16) is verified when present.
    let payload = deflate::deflate(b"hc", CompressionLevel::Normal);
    let mut member = vec![0x1F, 0x8B, 0x08, 0x02, 0, 0, 0, 0, 0, 0x03];
    let mut c = Crc32::new();
    c.update(&member);
    member.extend_from_slice(&((c.value() & 0xFFFF) as u16).to_le_bytes());
    member.extend_from_slice(&payload);
    member.extend_from_slice(&crc32(b"hc").to_le_bytes());
    member.extend_from_slice(&2u32.to_le_bytes());
    assert_eq!(gzip::decompress(&member).unwrap(), b"hc");
    member[10] ^= 0xFF; // corrupt the stored header CRC16
    assert!(matches!(
        gzip::decompress(&member),
        Err(GzipError::HeaderCrcMismatch { .. })
    ));
}

#[test]
fn gzip_trailer_verification() {
    let gz = gzip::compress(b"checksum me");
    let mut bad = gz.clone();
    let n = bad.len();
    bad[n - 8] ^= 0xFF; // stored CRC-32
    assert!(matches!(
        gzip::decompress(&bad),
        Err(GzipError::CrcMismatch { .. })
    ));
    let mut bad2 = gz;
    let n = bad2.len();
    bad2[n - 4] ^= 0xFF; // ISIZE
    assert!(matches!(
        gzip::decompress(&bad2),
        Err(GzipError::LengthMismatch { .. })
    ));
}

#[test]
fn gzip_multimember_stream() {
    let stream = [
        gzip::compress(b"alpha,"),
        gzip::compress(b"beta,"),
        gzip::compress(b"gamma"),
    ]
    .concat();
    assert_eq!(gzip::decompress(&stream).unwrap(), b"alpha,beta,gamma");
    // Streaming decoder across members with tiny reads.
    let mut dec = GzipDecoder::new(&stream[..]);
    let mut small = [0u8; 4];
    let mut got = Vec::new();
    loop {
        let n = dec.read(&mut small).unwrap();
        if n == 0 {
            break;
        }
        got.extend_from_slice(&small[..n]);
        assert!(dec.header().is_some());
    }
    dec.finish().unwrap();
    assert_eq!(got, b"alpha,beta,gamma");
    // Empty members are legal.
    let stream2 = [
        gzip::compress(b""),
        gzip::compress(b"x"),
        gzip::compress(b""),
    ]
    .concat();
    assert_eq!(gzip::decompress(&stream2).unwrap(), b"x");
}

#[test]
fn gzip_output_limits() {
    let gz = gzip::compress(&vec![0u8; 1 << 20]);
    assert!(gz.len() < 8_192);
    assert!(matches!(
        gzip::decompress_limited(&gz, 64 * 1024),
        Err(GzipError::Deflate(DeflateError::OutputLimitExceeded))
    ));
    assert_eq!(
        gzip::decompress_limited(&gz, 1 << 20).unwrap().len(),
        1 << 20
    );
    // The cap spans members.
    let two = [
        gzip::compress(&vec![0u8; 40_000]),
        gzip::compress(&vec![0u8; 40_000]),
    ]
    .concat();
    assert!(matches!(
        gzip::decompress_limited(&two, 50_000),
        Err(GzipError::Deflate(DeflateError::OutputLimitExceeded))
    ));
    assert_eq!(
        gzip::decompress_limited(&two, 80_000).unwrap().len(),
        80_000
    );
}

#[test]
fn gzip_bad_magic_and_method() {
    let mut bad = gzip::compress(b"x");
    bad[0] = 0x00;
    bad[1] = 0x11;
    assert!(matches!(
        gzip::decompress(&bad),
        Err(GzipError::BadMagic { .. })
    ));
    let mut m = gzip::compress(b"x");
    m[2] = 9;
    assert!(matches!(
        gzip::decompress(&m),
        Err(GzipError::UnsupportedMethod(9))
    ));
    assert!(matches!(gzip::decompress(b""), Err(GzipError::Truncated)));
    assert!(matches!(
        gzip::decompress(&[0x1F, 0x8B]),
        Err(GzipError::Truncated)
    ));
}

#[test]
fn gzip_streaming_encoder_matches_one_shot() {
    let data = lcg_bytes(150_000);
    let opts = GzipOptions::new().filename("f.bin").mtime(123);
    let one_shot = gzip::compress_with(&data, &opts);
    let mut enc = gzip::GzipEncoder::with_options(Vec::new(), &opts).unwrap();
    for piece in data.chunks(977) {
        enc.write_all(piece).unwrap();
    }
    let streamed = enc.into_inner().unwrap();
    assert_eq!(streamed, one_shot);
}

#[test]
fn gzip_streaming_decoder_finish_and_garbage() {
    let gz = gzip::compress(b"data");
    // Trailing garbage is detected by finish().
    let mut garbage = gz.clone();
    garbage.extend_from_slice(b"not-gzip!");
    let mut dec = GzipDecoder::new(&garbage[..]);
    let mut out = Vec::new();
    dec.read_to_end(&mut out).unwrap();
    assert_eq!(out, b"data");
    assert!(matches!(dec.finish(), Err(GzipError::TrailingGarbage)));
    // A single trailing byte is garbage too.
    let mut g1 = gz.clone();
    g1.push(0x42);
    let mut dec = GzipDecoder::new(&g1[..]);
    dec.read_to_end(&mut Vec::new()).unwrap();
    assert!(matches!(dec.finish(), Err(GzipError::TrailingGarbage)));
    // A truncated member errors during reading.
    let cut = &gz[..gz.len() - 4];
    let mut dec = GzipDecoder::new(cut);
    assert!(dec.read_to_end(&mut Vec::new()).is_err());
}

#[test]
fn gzip_interop_with_system_gzip() {
    if !has_tool("gzip") {
        eprintln!("skipping: system gzip not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    let data = lcg_bytes(64 * 1024);
    let plain = dir.path().join("plain");
    std::fs::write(&plain, &data).unwrap();
    // System gzip compresses; we decompress.
    let gz = run_tool("gzip", &["-c", plain.to_str().unwrap()], None).expect("gzip -c");
    assert_eq!(gzip::decompress(&gz).unwrap(), data);
    // We compress; system gzip decompresses and tests integrity.
    let ours = gzip::compress(&data);
    let ours_path = dir.path().join("ours.gz");
    std::fs::write(&ours_path, &ours).unwrap();
    let round = run_tool("gzip", &["-dc", ours_path.to_str().unwrap()], None).expect("gzip -dc");
    assert_eq!(round, data);
    let status = Command::new("gzip")
        .arg("-t")
        .arg(&ours_path)
        .status()
        .unwrap();
    assert!(status.success(), "gzip -t rejected our member");
}

#[test]
fn gzip_of_tar_roundtrip() {
    let mut w = TarWriter::new(Vec::new());
    w.append_file("a.txt", 0o644, 1, b"hello").unwrap();
    w.append_dir("d", 0o755, 1).unwrap();
    let tar_bytes = w.finish().unwrap();
    let gz = gzip::compress(&tar_bytes);
    let raw = gzip::decompress(&gz).unwrap();
    assert_eq!(raw, tar_bytes);
    let mut r = TarReader::new(&raw[..]);
    assert_eq!(r.next_entry().unwrap().unwrap().path(), "a.txt");
}

// ---------------------------------------------------------------------------
// Tar
// ---------------------------------------------------------------------------

/// Raw 512-byte tar header with no safety checks (for crafting archives).
fn raw_tar_header(name: &str, size: u64, typeflag: u8, link: Option<&str>) -> [u8; 512] {
    let mut h = [0u8; 512];
    let n = name.len().min(100);
    h[0..n].copy_from_slice(&name.as_bytes()[..n]);
    h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
    h[136..148].copy_from_slice(b"00000000001\0");
    h[156] = typeflag;
    if let Some(l) = link {
        let m = l.len().min(100);
        h[157..157 + m].copy_from_slice(&l.as_bytes()[..m]);
    }
    h[257..263].copy_from_slice(b"ustar\x00");
    h[263..265].copy_from_slice(b"00");
    let mut sum = 0u32;
    for (i, &b) in h.iter().enumerate() {
        sum += if (148..156).contains(&i) {
            b' ' as u32
        } else {
            b as u32
        };
    }
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    h
}

/// One tar entry (header + data + padding) for crafted archives.
fn tar_entry_bytes(name: &str, data: &[u8], typeflag: u8, link: Option<&str>) -> Vec<u8> {
    let mut out = raw_tar_header(name, data.len() as u64, typeflag, link).to_vec();
    out.extend_from_slice(data);
    let pad = (512 - data.len() % 512) % 512;
    out.extend(std::iter::repeat_n(0u8, pad));
    out
}

#[test]
fn tar_ustar_roundtrip_all_entry_types() {
    let mut w = TarWriter::new(Vec::new());
    w.append_file("hello.txt", 0o644, 1_700_000_000, b"hello tar\n")
        .unwrap();
    w.append_dir("sub", 0o755, 1_700_000_001).unwrap();
    w.append_file("sub/nested.bin", 0o600, 42, &lcg_bytes(1000))
        .unwrap();
    w.append_symlink("link", "hello.txt", 1).unwrap();
    w.append_hardlink("hard", "hello.txt", 1).unwrap();
    let bytes = w.finish().unwrap();

    let mut r = TarReader::new(&bytes[..]);
    let mut e = r.next_entry().unwrap().unwrap();
    assert_eq!(e.path(), "hello.txt");
    assert_eq!(e.entry_type(), EntryType::File);
    assert_eq!(e.size(), 10);
    assert_eq!(e.mode(), 0o644);
    assert_eq!(e.mtime(), 1_700_000_000);
    let mut data = Vec::new();
    e.read_to_end(&mut data).unwrap();
    assert_eq!(data, b"hello tar\n");

    let e = r.next_entry().unwrap().unwrap();
    assert_eq!((e.path(), e.entry_type()), ("sub", EntryType::Directory));

    let mut e = r.next_entry().unwrap().unwrap();
    assert_eq!(e.path(), "sub/nested.bin");
    assert_eq!(e.mode(), 0o600);
    let mut data = Vec::new();
    e.read_to_end(&mut data).unwrap();
    assert_eq!(data, lcg_bytes(1000));

    let e = r.next_entry().unwrap().unwrap();
    assert_eq!(e.entry_type(), EntryType::Symlink);
    assert_eq!(e.link_name(), Some("hello.txt"));

    let e = r.next_entry().unwrap().unwrap();
    assert_eq!(e.entry_type(), EntryType::HardLink);
    assert_eq!(e.link_name(), Some("hello.txt"));

    assert!(r.next_entry().unwrap().is_none());
}

#[test]
fn tar_pax_long_names() {
    // 185-byte path and 151-byte link name force PAX records.
    let long = format!("deep/{}", "a".repeat(180));
    let long_target = format!("b{}", "x".repeat(150));
    let mut w = TarWriter::new(Vec::new());
    w.append_file(&long, 0o644, 5, b"12345").unwrap();
    w.append_symlink(&long_target, &long_target, 1).unwrap();
    let bytes = w.finish().unwrap();
    let mut r = TarReader::new(&bytes[..]);
    let mut e = r.next_entry().unwrap().unwrap();
    assert_eq!(e.path(), long);
    let mut data = Vec::new();
    e.read_to_end(&mut data).unwrap();
    assert_eq!(data, b"12345");
    let e = r.next_entry().unwrap().unwrap();
    assert_eq!(e.path(), long_target);
    assert_eq!(e.link_name(), Some(long_target.as_str()));
    assert!(r.next_entry().unwrap().is_none());
    // USTAR name/prefix split boundaries.
    for n in [99usize, 100, 101] {
        let name = "n".repeat(n);
        let mut w = TarWriter::new(Vec::new());
        w.append_file(&name, 0o644, 0, b"").unwrap();
        let bytes = w.finish().unwrap();
        let mut r = TarReader::new(&bytes[..]);
        let e = r.next_entry().unwrap().unwrap();
        assert_eq!(e.path(), name);
    }
}

#[test]
fn tar_checksum_verification() {
    let mut w = TarWriter::new(Vec::new());
    w.append_file("f.txt", 0o644, 0, b"data").unwrap();
    let mut bytes = w.finish().unwrap();
    bytes[100] ^= 0x55; // flip a byte inside the header
    let mut r = TarReader::new(&bytes[..]);
    assert!(matches!(r.next_entry(), Err(TarError::BadChecksum { .. })));
}

#[test]
fn tar_writer_rejects_unsafe_paths() {
    let mut w = TarWriter::new(Vec::new());
    for bad in ["../evil.txt", "/abs.txt", "a/../../b", "a\\b", "x\0y"] {
        assert!(
            matches!(
                w.append_file(bad, 0o644, 0, b""),
                Err(TarError::Extract(ExtractError::UnsafePath(_)))
            ),
            "writer accepted unsafe path {bad:?}"
        );
    }
}

#[test]
fn tar_extract_rejects_traversal_and_absolute() {
    let dir = TempDir::new().unwrap();
    for name in ["../evil.txt", "/etc/pwn", "C:/x", "a/../../b"] {
        let mut archive = tar_entry_bytes(name, b"pwn", b'0', None);
        archive.extend_from_slice(&[0u8; 1024]);
        let mut r = TarReader::new(&archive[..]);
        let err = r.safe_extract_to(dir.path(), &ExtractOptions::new());
        assert!(
            matches!(err, Err(TarError::Extract(ExtractError::UnsafePath(_)))),
            "extractor accepted unsafe path {name:?}"
        );
        assert_eq!(dir.path().read_dir().unwrap().count(), 0);
    }
}

#[cfg(unix)]
#[test]
fn tar_extract_rejects_symlink_escape() {
    // (a) archive-planted symlink pointing outside.
    let dir = TempDir::new().unwrap();
    let mut archive = tar_entry_bytes("out", b"", b'2', Some("../../outside"));
    archive.extend_from_slice(&[0u8; 1024]);
    let mut r = TarReader::new(&archive[..]);
    assert!(matches!(
        r.safe_extract_to(dir.path(), &ExtractOptions::new()),
        Err(TarError::Extract(ExtractError::SymlinkEscape(_)))
    ));
    // (b) absolute link target.
    let mut archive = tar_entry_bytes("abs", b"", b'2', Some("/etc/passwd"));
    archive.extend_from_slice(&[0u8; 1024]);
    let mut r = TarReader::new(&archive[..]);
    assert!(matches!(
        r.safe_extract_to(dir.path(), &ExtractOptions::new()),
        Err(TarError::Extract(ExtractError::SymlinkEscape(_)))
    ));
    // (c) pre-existing symlink inside dest_root blocks extraction through it.
    let outside = TempDir::new().unwrap();
    let esc = dir.path().join("esc");
    std::os::unix::fs::symlink(outside.path(), &esc).unwrap();
    let mut archive = tar_entry_bytes("esc/data.txt", b"pwn", b'0', None);
    archive.extend_from_slice(&[0u8; 1024]);
    let mut r = TarReader::new(&archive[..]);
    assert!(matches!(
        r.safe_extract_to(dir.path(), &ExtractOptions::new()),
        Err(TarError::Extract(ExtractError::SymlinkEscape(_)))
    ));
    assert!(!outside.path().join("data.txt").exists());
    // (d) a relative in-root symlink is fine.
    let mut archive = tar_entry_bytes("base.txt", b"root", b'0', None);
    archive.extend(tar_entry_bytes("d", b"", b'5', None));
    archive.extend(tar_entry_bytes("d/ok", b"", b'2', Some("../base.txt")));
    archive.extend_from_slice(&[0u8; 1024]);
    let dir2 = TempDir::new().unwrap();
    let mut r = TarReader::new(&archive[..]);
    assert_eq!(
        r.safe_extract_to(dir2.path(), &ExtractOptions::new())
            .unwrap(),
        3
    );
    assert_eq!(
        std::fs::read(dir2.path().join("base.txt")).unwrap(),
        b"root"
    );
    assert!(dir2.path().join("d/ok").exists());
}

#[test]
fn tar_extract_success_summary_and_overwrite() {
    let dir = TempDir::new().unwrap();
    let mut archive = tar_entry_bytes("data/", b"", b'5', None);
    archive.extend(tar_entry_bytes("data/a.txt", b"AAA", b'0', None));
    archive.extend(tar_entry_bytes("data/b.bin", &lcg_bytes(700), b'0', None));
    archive.extend(tar_entry_bytes("fifo", b"", b'6', None)); // skipped, not written
    archive.extend_from_slice(&[0u8; 1024]);
    let mut r = TarReader::new(&archive[..]);
    let n = r
        .safe_extract_to(dir.path(), &ExtractOptions::new())
        .unwrap();
    assert_eq!(n, 3); // dir + two files
    assert_eq!(
        std::fs::read(dir.path().join("data/a.txt")).unwrap(),
        b"AAA"
    );
    assert_eq!(
        std::fs::read(dir.path().join("data/b.bin")).unwrap(),
        lcg_bytes(700)
    );
    // No overwrite: the second run collides.
    let mut r = TarReader::new(&archive[..]);
    assert!(matches!(
        r.safe_extract_to(dir.path(), &ExtractOptions::new()),
        Err(TarError::Extract(ExtractError::DestinationExists(_)))
    ));
    // Overwrite flag rewrites cleanly.
    let mut opts = ExtractOptions::new();
    opts.overwrite_existing = true;
    let mut r = TarReader::new(&archive[..]);
    assert_eq!(r.safe_extract_to(dir.path(), &opts).unwrap(), 3);
}

#[test]
fn tar_extraction_size_limits() {
    let dir = TempDir::new().unwrap();
    // Per-entry cap.
    let mut archive = tar_entry_bytes("big.txt", &vec![0u8; 10_000], b'0', None);
    archive.extend_from_slice(&[0u8; 1024]);
    let mut opts = ExtractOptions::new();
    opts.max_entry_bytes = 4_000;
    let mut r = TarReader::new(&archive[..]);
    assert!(matches!(
        r.safe_extract_to(dir.path(), &opts),
        Err(TarError::Extract(ExtractError::EntryTooLarge { .. }))
    ));
    // Total cap.
    let mut archive = tar_entry_bytes("a", &vec![0u8; 3_000], b'0', None);
    archive.extend(tar_entry_bytes("b", &vec![0u8; 3_000], b'0', None));
    archive.extend_from_slice(&[0u8; 1024]);
    let mut opts = ExtractOptions::new();
    opts.max_total_bytes = 5_000;
    let mut r = TarReader::new(&archive[..]);
    assert!(matches!(
        r.safe_extract_to(dir.path(), &opts),
        Err(TarError::Extract(ExtractError::TotalTooLarge { .. }))
    ));
}

#[test]
fn tar_truncated_archive_rejected() {
    let mut w = TarWriter::new(Vec::new());
    w.append_file("f", 0o644, 0, b"12345").unwrap();
    let bytes = w.finish().unwrap();
    // Cut inside the data/pad region: the entry reads short, the next
    // header read reports truncation.
    let mut r = TarReader::new(&bytes[..520]);
    let mut e = r.next_entry().unwrap().unwrap();
    let mut buf = Vec::new();
    let _ = e.read_to_end(&mut buf);
    assert!(buf.len() <= 5);
    assert!(matches!(r.next_entry(), Err(TarError::Truncated)));
    // Cut inside the header itself.
    let mut r = TarReader::new(&bytes[..300]);
    assert!(matches!(r.next_entry(), Err(TarError::Truncated)));
    // EOF without end markers is accepted (single zero block or EOF).
    let mut r = TarReader::new(&bytes[..1024]);
    assert!(r.next_entry().unwrap().is_some());
    assert!(r.next_entry().unwrap().is_none());
}

#[test]
fn tar_interop_with_system_tar() {
    if !has_tool("tar") {
        eprintln!("skipping: system tar not available");
        return;
    }
    let dir = TempDir::new().unwrap();
    // Ours -> system tar (list + extract).
    let mut w = TarWriter::new(Vec::new());
    w.append_file("one.txt", 0o644, 60, b"interop body\n")
        .unwrap();
    w.append_dir("d", 0o755, 60).unwrap();
    w.append_file("d/two.txt", 0o644, 60, b"two\n").unwrap();
    let tar_bytes = w.finish().unwrap();
    let tar_path = dir.path().join("a.tar");
    std::fs::write(&tar_path, &tar_bytes).unwrap();
    let listing = run_tool("tar", &["-tf", tar_path.to_str().unwrap()], None).expect("tar -tf");
    let names = String::from_utf8_lossy(&listing);
    assert!(names.contains("one.txt"), "listing: {names}");
    assert!(names.contains("d/two.txt"), "listing: {names}");
    let outdir = dir.path().join("x");
    std::fs::create_dir(&outdir).unwrap();
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&tar_path)
        .current_dir(&outdir)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(std::fs::read(outdir.join("d/two.txt")).unwrap(), b"two\n");
    // System tar -> ours: a >100-byte name forces GNU longname or PAX.
    let src = dir.path().join("src");
    std::fs::create_dir(&src).unwrap();
    let long = format!("{}.txt", "L".repeat(150));
    std::fs::write(src.join(&long), b"long!").unwrap();
    std::fs::write(src.join("short.txt"), b"short!").unwrap();
    let sys_path = dir.path().join("sys.tar");
    let status = Command::new("tar")
        .arg("-cf")
        .arg(&sys_path)
        .arg("-C")
        .arg(&src)
        .arg(".")
        .status()
        .unwrap();
    assert!(status.success());
    let bytes = std::fs::read(&sys_path).unwrap();
    let mut r = TarReader::new(&bytes[..]);
    let mut found_long = false;
    let mut found_short = false;
    while let Some(e) = r.next_entry().unwrap() {
        if e.path().ends_with(&long) {
            found_long = true;
        }
        if e.path().ends_with("short.txt") {
            found_short = true;
        }
    }
    assert!(found_long, "our reader must see the long name");
    assert!(found_short);
}

// ---------------------------------------------------------------------------
// Zip
// ---------------------------------------------------------------------------

/// Hand-built single-entry zip (method passed explicitly) for crafted cases.
fn raw_zip(name: &str, data: &[u8], method: u16) -> Vec<u8> {
    let crc = crc32(data);
    let mut out = Vec::new();
    out.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
    out.extend_from_slice(&20u16.to_le_bytes());
    out.extend_from_slice(&0x0800u16.to_le_bytes());
    out.extend_from_slice(&method.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // time
    out.extend_from_slice(&0x21u16.to_le_bytes()); // date (1980-01-01)
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // extra len
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(data);
    let cd_off = out.len() as u32;
    out.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]);
    out.extend_from_slice(&(((3u16) << 8) | 20u16).to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes());
    out.extend_from_slice(&0x0800u16.to_le_bytes());
    out.extend_from_slice(&method.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0x21u16.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // extra len
    out.extend_from_slice(&0u16.to_le_bytes()); // comment len
    out.extend_from_slice(&0u16.to_le_bytes()); // disk start
    out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
    out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
    out.extend_from_slice(&0u32.to_le_bytes()); // local header offset (entry starts at 0)
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06]);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&((46 + name.len()) as u32).to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[test]
fn zip_store_and_deflate_roundtrip() {
    let cases: Vec<Vec<u8>> = vec![
        Vec::new(),
        b"x".to_vec(),
        lcg_bytes(64 * 1024),
        lorem(400),
        vec![0u8; 100_000],
    ];
    for data in &cases {
        for level in [
            CompressionLevel::Stored,
            CompressionLevel::Normal,
            CompressionLevel::Best,
        ] {
            let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
            w.add_file("entry.bin", data, level).unwrap();
            let bytes = w.finish().unwrap().into_inner();
            let mut r = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
            let e = r.entry_by_name("entry.bin").cloned().unwrap();
            assert_eq!(e.size(), data.len() as u64);
            assert_eq!(e.crc32(), crc32(data));
            assert_eq!(r.read_entry(&e, 1 << 20).unwrap(), *data);
        }
    }
    // Compressible data really uses Deflate and shrinks.
    let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.add_file("lorem", &lorem(400), CompressionLevel::Best)
        .unwrap();
    let bytes = w.finish().unwrap().into_inner();
    let r = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
    let e = r.entry_by_name("lorem").unwrap();
    assert_eq!(e.method(), Method::Deflate);
    assert!(e.compressed_size() < e.size());
    // Incompressible data auto-falls back to Store.
    let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.add_file("rand", &lcg_bytes(64 * 1024), CompressionLevel::Best)
        .unwrap();
    let bytes = w.finish().unwrap().into_inner();
    let r = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(r.entry_by_name("rand").unwrap().method(), Method::Store);
}

#[test]
fn zip_dirs_and_mixed_methods() {
    let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.add_dir("docs").unwrap();
    w.add_file("docs/a.txt", b"aaa", CompressionLevel::Stored)
        .unwrap();
    w.add_file("docs/b.txt", b"bbb bbb bbb bbb", CompressionLevel::Normal)
        .unwrap();
    w.add_file("root.bin", &[7u8; 100], CompressionLevel::Best)
        .unwrap();
    let bytes = w.finish().unwrap().into_inner();
    let mut r = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(r.entries().len(), 4);
    assert!(r.entry_by_name("docs/").unwrap().is_dir());
    assert_eq!(
        r.entry_by_name("docs/b.txt").unwrap().method(),
        Method::Deflate
    );
    assert_eq!(
        r.read_entry_by_name("root.bin", 1024).unwrap(),
        vec![7u8; 100]
    );
    assert_eq!(r.read_entry_by_name("docs/a.txt", 1024).unwrap(), b"aaa");
    assert!(r.read_entry_by_name("missing", 1024).is_err());
}

#[test]
fn zip_eocd_backward_scan_with_comment() {
    let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.add_file("a", b"data", CompressionLevel::Stored).unwrap();
    let mut bytes = w.finish().unwrap().into_inner();
    // Append a zip comment and patch the EOCD comment-length field.
    let comment = vec![b'c'; 100];
    let n = bytes.len();
    bytes.extend_from_slice(&comment);
    let len_bytes = (comment.len() as u16).to_le_bytes();
    bytes[n - 2] = len_bytes[0];
    bytes[n - 1] = len_bytes[1];
    let r = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(r.entries().len(), 1);
    // Random garbage is not a zip at all.
    assert!(matches!(
        ZipReader::new(std::io::Cursor::new(lcg_bytes(500))),
        Err(ZipError::NotAZipArchive)
    ));
}

#[test]
fn zip_crc_and_size_mismatch_rejected() {
    let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.add_file("stored.bin", &lcg_bytes(300), CompressionLevel::Stored)
        .unwrap();
    let mut bytes = w.finish().unwrap().into_inner();
    // Corrupt one stored data byte (local header is 30 + name bytes).
    let data_off = 30 + "stored.bin".len();
    bytes[data_off] ^= 0xFF;
    let mut r = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
    let e = r.entry_by_name("stored.bin").cloned().unwrap();
    assert!(matches!(
        r.read_entry(&e, 1024),
        Err(ZipError::CrcMismatch { .. })
    ));
    // Patch the central-directory uncompressed size -> SizeMismatch.
    let bytes = raw_zip("stored.bin", &lcg_bytes(300), 0);
    let mut bytes = bytes;
    let cd_at = bytes
        .windows(4)
        .rposition(|w| w == [0x50, 0x4B, 0x01, 0x02])
        .unwrap();
    bytes[cd_at + 24] = 43; // low byte: 300 (0x2C 01) -> 299 (0x2B 01)
    let mut r = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
    let e = r.entry_by_name("stored.bin").cloned().unwrap();
    assert!(matches!(
        r.read_entry(&e, 1024),
        Err(ZipError::SizeMismatch { .. })
    ));
}

#[test]
fn zip_slip_rejected_but_readable() {
    let dir = TempDir::new().unwrap();
    for name in ["../evil.txt", "/abs.txt", "a/../../b"] {
        let bytes = raw_zip(name, b"pwn", 0);
        let mut r = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
        // Raw reads have no path checks.
        assert_eq!(r.read_entry_by_name(name, 64).unwrap(), b"pwn");
        // Extraction refuses.
        let err = r.safe_extract_to(dir.path(), &ExtractOptions::new());
        assert!(
            matches!(err, Err(ZipError::Extract(ExtractError::UnsafePath(_)))),
            "extractor accepted unsafe path {name:?}"
        );
    }
    assert_eq!(dir.path().read_dir().unwrap().count(), 0);
}

#[test]
fn zip_bomb_guards() {
    let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.add_file("bomb", &vec![0u8; 1 << 20], CompressionLevel::Best)
        .unwrap();
    let bytes = w.finish().unwrap().into_inner();
    let mut r = ZipReader::new(std::io::Cursor::new(bytes.clone())).unwrap();
    let e = r.entry_by_name("bomb").cloned().unwrap();
    assert!(e.compressed_size() < 4_096);
    // Read cap.
    assert!(matches!(
        r.read_entry(&e, 64 * 1024),
        Err(ZipError::Deflate(DeflateError::OutputLimitExceeded))
    ));
    assert_eq!(r.read_entry(&e, 1 << 20).unwrap().len(), 1 << 20);
    // Extraction cap.
    let dir = TempDir::new().unwrap();
    let mut opts = ExtractOptions::new();
    opts.max_entry_bytes = 64 * 1024;
    let mut r = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
    assert!(matches!(
        r.safe_extract_to(dir.path(), &opts),
        Err(ZipError::Extract(ExtractError::EntryTooLarge { .. }))
    ));
}

#[test]
fn zip_unsupported_features_rejected() {
    // Unsupported method (12).
    let bytes = raw_zip("m.bin", b"data", 12);
    let mut r = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
    let e = r.entry_by_name("m.bin").cloned().unwrap();
    assert!(matches!(
        r.read_entry(&e, 64),
        Err(ZipError::UnsupportedMethod(12))
    ));
    // Encrypted entries (general-purpose flag bit 0) in local + central.
    let mut bytes = raw_zip("enc.bin", b"data", 0);
    bytes[6] |= 0x01;
    let cd_at = bytes
        .windows(4)
        .rposition(|w| w == [0x50, 0x4B, 0x01, 0x02])
        .unwrap();
    bytes[cd_at + 8] |= 0x01;
    let mut r = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
    let e = r.entry_by_name("enc.bin").cloned().unwrap();
    assert!(matches!(
        r.read_entry(&e, 64),
        Err(ZipError::UnsupportedEncryption)
    ));
    // Zip64 sentinel in the entry count.
    let mut bytes = raw_zip("z.bin", b"d", 0);
    let eocd_at = bytes
        .windows(4)
        .rposition(|w| w == [0x50, 0x4B, 0x05, 0x06])
        .unwrap();
    bytes[eocd_at + 8] = 0xFF; // disk entries -> Zip64 sentinel
    bytes[eocd_at + 9] = 0xFF;
    bytes[eocd_at + 10] = 0xFF; // total entries -> Zip64 sentinel
    bytes[eocd_at + 11] = 0xFF;
    assert!(matches!(
        ZipReader::new(std::io::Cursor::new(bytes)),
        Err(ZipError::UnsupportedZip64)
    ));
}

#[test]
fn zip_malformed_archives_rejected() {
    assert!(matches!(
        ZipReader::new(std::io::Cursor::new(lcg_bytes(500))),
        Err(ZipError::NotAZipArchive)
    ));
    // Truncations: EOCD or central directory no longer intact.
    let bytes = raw_zip("a.txt", b"hello", 0);
    for cut in [bytes.len() / 3, bytes.len() / 2] {
        let res = ZipReader::new(std::io::Cursor::new(&bytes[..cut]));
        assert!(matches!(
            res,
            Err(ZipError::NotAZipArchive)
                | Err(ZipError::CorruptCentralDirectory)
                | Err(ZipError::Truncated)
                | Err(ZipError::UnsupportedZip64)
        ));
    }
    // Corrupt central-directory signature.
    let mut bytes = raw_zip("a.txt", b"hello", 0);
    let cd_at = bytes
        .windows(4)
        .rposition(|w| w == [0x50, 0x4B, 0x01, 0x02])
        .unwrap();
    bytes[cd_at] = 0x99;
    assert!(matches!(
        ZipReader::new(std::io::Cursor::new(bytes)),
        Err(ZipError::CorruptCentralDirectory)
    ));
}

#[test]
fn zip_interop_with_system_tools() {
    if !has_tool("python3") && !has_tool("unzip") {
        eprintln!("skipping: no unzip/python3 in the container (alpine busybox only)");
        return;
    }
    let dir = TempDir::new().unwrap();
    let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.add_file("a.txt", b"hello zip\n", CompressionLevel::Stored)
        .unwrap();
    w.add_file("b.txt", b"deflate me, deflate me\n", CompressionLevel::Best)
        .unwrap();
    let bytes = w.finish().unwrap().into_inner();
    let zip_path = dir.path().join("a.zip");
    std::fs::write(&zip_path, &bytes).unwrap();
    if has_tool("python3") {
        let outdir = dir.path().join("py");
        std::fs::create_dir(&outdir).unwrap();
        let status = Command::new("python3")
            .arg("-m")
            .arg("zipfile")
            .arg("-e")
            .arg(&zip_path)
            .arg(&outdir)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(
            std::fs::read(outdir.join("b.txt")).unwrap(),
            b"deflate me, deflate me\n"
        );
        // Read a python-created zip back.
        let src = dir.path().join("src.pyz");
        let status = Command::new("python3")
            .arg("-m")
            .arg("zipfile")
            .arg("-c")
            .arg(&src)
            .arg("hello.txt")
            .arg(zip_path.to_str().unwrap())
            .status();
        // The -c path needs a real file argument; skip silently otherwise.
        if status.map(|s| s.success()).unwrap_or(false) {
            let bytes = std::fs::read(&src).unwrap();
            let r = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
            assert!(r.entry_by_name("hello.txt").is_some());
        }
    } else if has_tool("unzip") {
        let outdir = dir.path().join("uz");
        std::fs::create_dir(&outdir).unwrap();
        let status = Command::new("unzip")
            .arg("-o")
            .arg(&zip_path)
            .arg("-d")
            .arg(&outdir)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(std::fs::read(outdir.join("a.txt")).unwrap(), b"hello zip\n");
    }
}

// ---------------------------------------------------------------------------
// Regression tests for verified audit findings (batch B1)
// ---------------------------------------------------------------------------

/// A reader that hands out data in chunks whose sizes come from `sizes`
/// (cycled), simulating arbitrary network/pipe boundaries.
struct ChunkedReader<'a> {
    data: &'a [u8],
    pos: usize,
    sizes: Vec<usize>,
    turn: usize,
}

impl<'a> ChunkedReader<'a> {
    fn new(data: &'a [u8], sizes: Vec<usize>) -> Self {
        Self {
            data,
            pos: 0,
            sizes,
            turn: 0,
        }
    }
}

impl Read for ChunkedReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let want = self.sizes[self.turn % self.sizes.len()].max(1);
        self.turn += 1;
        let n = want.min(buf.len()).min(self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Pseudo-random chunk sizes in `1..=max`.
fn random_sizes(count: usize, max: usize) -> Vec<usize> {
    lcg_bytes(count * 2)
        .chunks(2)
        .map(|c| (u16::from_le_bytes([c[0], c[1]]) as usize % max) + 1)
        .collect()
}

/// >100 KiB of mixed compressible and incompressible data.
fn mixed_payload() -> Vec<u8> {
    let mut data = lorem(1_500);
    data.extend_from_slice(&lcg_bytes(40_000));
    data.extend_from_slice(&lorem(1_000));
    data
}

#[cfg(unix)]
#[test]
fn tar_symlink_target_resolved_from_parent_dir() {
    // `d/l -> ../../x` used to be resolved from `d/l` (one level too deep),
    // so it was accepted although the OS resolves it to `<root>/../x`.
    for (name, target) in [
        ("d/l", "../../outside"),
        ("d/e/l", "../../../x"),
        ("l", "../x"),
        // `..` after a normal component may hop through a symlink.
        ("m", "d/s/../../../outside"),
        ("n", "d/../d/../.."),
    ] {
        let dir = TempDir::new().unwrap();
        let mut archive = tar_entry_bytes("d", b"", b'5', None);
        archive.extend(tar_entry_bytes("d/e", b"", b'5', None));
        archive.extend(tar_entry_bytes(name, b"", b'2', Some(target)));
        archive.extend_from_slice(&[0u8; 1024]);
        let mut r = TarReader::new(&archive[..]);
        let res = r.safe_extract_to(dir.path(), &ExtractOptions::new());
        assert!(
            matches!(res, Err(TarError::Extract(ExtractError::SymlinkEscape(_)))),
            "symlink {name} -> {target} was accepted: {res:?}"
        );
        assert!(
            std::fs::symlink_metadata(dir.path().join(name)).is_err(),
            "escaping symlink {name} was created"
        );
    }
    // In-root relative links (including one pointing at the root) still work.
    let dir = TempDir::new().unwrap();
    let mut archive = tar_entry_bytes("base.txt", b"root", b'0', None);
    archive.extend(tar_entry_bytes("d", b"", b'5', None));
    archive.extend(tar_entry_bytes("d/up", b"", b'2', Some("..")));
    archive.extend(tar_entry_bytes("d/ok", b"", b'2', Some("./../base.txt")));
    archive.extend_from_slice(&[0u8; 1024]);
    let mut r = TarReader::new(&archive[..]);
    assert_eq!(
        r.safe_extract_to(dir.path(), &ExtractOptions::new())
            .unwrap(),
        4
    );
    assert_eq!(std::fs::read(dir.path().join("d/ok")).unwrap(), b"root");
}

#[cfg(unix)]
#[test]
fn tar_hardlink_never_aliases_through_symlinks() {
    let outside = TempDir::new().unwrap();
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, b"host data").unwrap();

    // (a) A symlink inside dest pointing at a host file, then a hard link to
    // that symlink, then a file entry overwriting the hard link.
    let dir = TempDir::new().unwrap();
    std::os::unix::fs::symlink(&secret, dir.path().join("esc")).unwrap();
    let mut archive = tar_entry_bytes("h", b"", b'1', Some("esc"));
    archive.extend(tar_entry_bytes("h", b"pwned", b'0', None));
    archive.extend_from_slice(&[0u8; 1024]);
    let mut opts = ExtractOptions::new();
    opts.overwrite_existing = true;
    let mut r = TarReader::new(&archive[..]);
    let res = r.safe_extract_to(dir.path(), &opts);
    assert!(
        matches!(res, Err(TarError::Extract(ExtractError::SymlinkEscape(_)))),
        "hard link to a symlink was accepted: {res:?}"
    );
    assert_eq!(std::fs::read(&secret).unwrap(), b"host data");
    assert!(std::fs::symlink_metadata(dir.path().join("h")).is_err());

    // (b) Hard-link source passing through a symlinked directory.
    let dir = TempDir::new().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("out")).unwrap();
    let mut archive = tar_entry_bytes("h", b"", b'1', Some("out/secret.txt"));
    archive.extend_from_slice(&[0u8; 1024]);
    let mut r = TarReader::new(&archive[..]);
    let res = r.safe_extract_to(dir.path(), &opts);
    assert!(
        matches!(res, Err(TarError::Extract(ExtractError::SymlinkEscape(_)))),
        "hard link through a symlinked dir was accepted: {res:?}"
    );
    assert_eq!(std::fs::read(&secret).unwrap(), b"host data");

    // (c) Archive-planted in-root symlink, then a hard link to it.
    let dir = TempDir::new().unwrap();
    let mut archive = tar_entry_bytes("base.txt", b"root", b'0', None);
    archive.extend(tar_entry_bytes("l", b"", b'2', Some("base.txt")));
    archive.extend(tar_entry_bytes("h", b"", b'1', Some("l")));
    archive.extend_from_slice(&[0u8; 1024]);
    let mut r = TarReader::new(&archive[..]);
    assert!(matches!(
        r.safe_extract_to(dir.path(), &ExtractOptions::new()),
        Err(TarError::Extract(ExtractError::SymlinkEscape(_)))
    ));

    // (d) `..` and absolute hard-link sources are refused; directories too.
    for target in ["../secret.txt", "/etc/passwd", "d"] {
        let dir = TempDir::new().unwrap();
        let mut archive = tar_entry_bytes("d", b"", b'5', None);
        archive.extend(tar_entry_bytes("h", b"", b'1', Some(target)));
        archive.extend_from_slice(&[0u8; 1024]);
        let mut r = TarReader::new(&archive[..]);
        let res = r.safe_extract_to(dir.path(), &ExtractOptions::new());
        assert!(
            matches!(
                res,
                Err(TarError::Extract(
                    ExtractError::UnsafePath(_) | ExtractError::LinkTargetMissing(_)
                ))
            ),
            "hard link to {target:?} was accepted: {res:?}"
        );
    }

    // (e) A regular in-root hard link still works.
    let dir = TempDir::new().unwrap();
    let mut archive = tar_entry_bytes("base.txt", b"root", b'0', None);
    archive.extend(tar_entry_bytes("h", b"", b'1', Some("base.txt")));
    archive.extend_from_slice(&[0u8; 1024]);
    let mut r = TarReader::new(&archive[..]);
    assert_eq!(
        r.safe_extract_to(dir.path(), &ExtractOptions::new())
            .unwrap(),
        2
    );
    assert_eq!(std::fs::read(dir.path().join("h")).unwrap(), b"root");
}

#[test]
fn tar_pax_length_overflow_is_an_error() {
    for payload in [
        format!("{} path=a\n", usize::MAX),
        format!("{} path=a\n", usize::MAX - 3),
        format!("11 path=a\n{} path=b\n", usize::MAX),
        "3 \n".to_string(),
        "1 x".to_string(),
    ] {
        let mut archive = tar_entry_bytes("pax", payload.as_bytes(), b'x', None);
        archive.extend(tar_entry_bytes("f.txt", b"hi", b'0', None));
        archive.extend_from_slice(&[0u8; 1024]);
        let mut r = TarReader::new(&archive[..]);
        let res = r.next_entry().map(|e| e.map(|e| e.path().to_string()));
        assert!(
            matches!(res, Err(TarError::InvalidPaxRecord(_))),
            "PAX payload {payload:?} gave {res:?}"
        );
    }
    // A huge PAX size override never overflows skipping arithmetic.
    let pax = format!("29 size={}\n", u64::MAX);
    assert_eq!(pax.len(), 29);
    let mut archive = tar_entry_bytes("pax", pax.as_bytes(), b'x', None);
    archive.extend(tar_entry_bytes("f.txt", b"hi", b'0', None));
    archive.extend_from_slice(&[0u8; 1024]);
    let mut r = TarReader::new(&archive[..]);
    assert_eq!(r.next_entry().unwrap().unwrap().size(), u64::MAX);
    assert!(matches!(r.next_entry(), Err(TarError::Truncated)));
}

/// Offset of the (single) central-directory record in a `raw_zip` archive.
fn cd_offset(bytes: &[u8]) -> usize {
    bytes
        .windows(4)
        .rposition(|w| w == [0x50, 0x4B, 0x01, 0x02])
        .unwrap()
}

#[test]
fn zip_central_directory_overrun_is_an_error() {
    // count = 2 while the first record's extra length runs past the CD:
    // `cd.len() - pos` used to underflow and panic.
    for extra in [1u16, 100, u16::MAX] {
        let mut bytes = raw_zip("a.txt", b"hello", 0);
        let cd = cd_offset(&bytes);
        bytes[cd + 30..cd + 32].copy_from_slice(&extra.to_le_bytes());
        let eocd = bytes.len() - 22;
        bytes[eocd + 8..eocd + 10].copy_from_slice(&2u16.to_le_bytes());
        bytes[eocd + 10..eocd + 12].copy_from_slice(&2u16.to_le_bytes());
        assert!(matches!(
            ZipReader::new(std::io::Cursor::new(bytes)),
            Err(ZipError::CorruptCentralDirectory)
        ));
    }
    // A huge declared entry count with a tiny CD is rejected without
    // trusting the count for allocation.
    let mut bytes = raw_zip("a.txt", b"hello", 0);
    let eocd = bytes.len() - 22;
    bytes[eocd + 8..eocd + 10].copy_from_slice(&0xFFFEu16.to_le_bytes());
    bytes[eocd + 10..eocd + 12].copy_from_slice(&0xFFFEu16.to_le_bytes());
    assert!(matches!(
        ZipReader::new(std::io::Cursor::new(bytes)),
        Err(ZipError::CorruptCentralDirectory)
    ));
}

#[test]
fn zip_declared_sizes_checked_before_allocation() {
    let huge = 0xFFFF_FFF0u32;
    // (a) Store entry declaring ~4 GiB: refused by the cap before any read.
    let mut bytes = raw_zip("a.txt", b"hello", 0);
    let cd = cd_offset(&bytes);
    bytes[cd + 20..cd + 24].copy_from_slice(&huge.to_le_bytes());
    bytes[cd + 24..cd + 28].copy_from_slice(&huge.to_le_bytes());
    let mut r = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
    assert!(matches!(
        r.read_entry_by_name("a.txt", 1 << 20),
        Err(ZipError::Deflate(DeflateError::OutputLimitExceeded))
    ));
    let dir = TempDir::new().unwrap();
    let mut opts = ExtractOptions::new();
    opts.max_entry_bytes = 1 << 20;
    assert!(matches!(
        r.safe_extract_to(dir.path(), &opts),
        Err(ZipError::Extract(ExtractError::EntryTooLarge { .. }))
    ));

    // (b) Deflate entry with a small size but a lying ~4 GiB compressed
    // size and a generous cap: read incrementally, fails as truncated.
    let packed = deflate::deflate(b"hello", CompressionLevel::Normal);
    let mut bytes = raw_zip("b.txt", &packed, 8);
    let cd = cd_offset(&bytes);
    bytes[cd + 16..cd + 20].copy_from_slice(&crc32(b"hello").to_le_bytes());
    bytes[cd + 20..cd + 24].copy_from_slice(&huge.to_le_bytes());
    bytes[cd + 24..cd + 28].copy_from_slice(&5u32.to_le_bytes());
    let mut r = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
    assert!(matches!(
        r.read_entry_by_name("b.txt", u64::MAX),
        Err(ZipError::Truncated)
    ));

    // (c) Store entry whose sizes disagree is refused before reading.
    let mut bytes = raw_zip("c.txt", b"hello", 0);
    let cd = cd_offset(&bytes);
    bytes[cd + 20..cd + 24].copy_from_slice(&huge.to_le_bytes());
    let mut r = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
    assert!(matches!(
        r.read_entry_by_name("c.txt", u64::MAX),
        Err(ZipError::SizeMismatch { .. })
    ));
}

#[test]
fn deflate_streaming_decoder_arbitrary_chunk_boundaries() {
    let data = mixed_payload();
    assert!(data.len() > 100 * 1024);
    for level in [
        CompressionLevel::Stored,
        CompressionLevel::Fast,
        CompressionLevel::Normal,
        CompressionLevel::Best,
    ] {
        let packed = deflate::deflate(&data, level);
        assert_eq!(deflate::inflate(&packed, 1 << 24).unwrap(), data);
        let schedules: Vec<Vec<usize>> =
            vec![vec![1], random_sizes(257, 4096), random_sizes(97, 70_000)];
        for sizes in schedules {
            let one_byte = sizes == [1];
            let reader = ChunkedReader::new(&packed, sizes);
            let mut dec = InflateDecoder::new(reader, 1 << 24);
            let mut out = Vec::new();
            if one_byte {
                // Also drain through a tiny output buffer.
                let mut buf = [0u8; 7];
                loop {
                    let n = dec.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    out.extend_from_slice(&buf[..n]);
                }
            } else {
                dec.read_to_end(&mut out).unwrap();
            }
            dec.finish().unwrap();
            assert!(out == data, "{level:?}: chunked decode differs");
        }
        // Raw push/pull with random chunk boundaries, keeping the tail.
        let sizes = random_sizes(301, 5000);
        let mut stream = InflateStream::new(1 << 24);
        let mut out = Vec::new();
        let (mut fed, mut avail, mut turn) = (0usize, 0usize, 0usize);
        let mut buf = vec![0u8; 3000];
        let mut rounds = 0usize;
        while !stream.is_finished() {
            rounds += 1;
            assert!(rounds < 1_000_000, "push/pull made no progress");
            if avail < packed.len() {
                avail = (avail + sizes[turn % sizes.len()]).min(packed.len());
                turn += 1;
            }
            fed += stream.push(&packed[fed..avail]).unwrap();
            loop {
                let n = stream.pull(&mut buf);
                if n == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..n]);
            }
        }
        assert!(out == data, "{level:?}: push/pull decode differs");
    }
    // Truncation at any chunking is still reported (no hang).
    let packed = deflate::deflate(&data, CompressionLevel::Best);
    let cut = &packed[..packed.len() - 3];
    let mut dec = InflateDecoder::new(ChunkedReader::new(cut, vec![1]), 1 << 24);
    let err = dec.read_to_end(&mut Vec::new()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
}

#[test]
fn gzip_streaming_decoder_arbitrary_chunk_boundaries() {
    let data = mixed_payload();
    let gz = [
        gzip::compress_with(&data, &GzipOptions::new().filename("a.bin")),
        gzip::compress_with(
            &data[..50_000],
            &GzipOptions::new().level(CompressionLevel::Best),
        ),
    ]
    .concat();
    let expected = [&data[..], &data[..50_000]].concat();
    assert_eq!(gzip::decompress(&gz).unwrap(), expected);
    for sizes in [vec![1], random_sizes(211, 3000), random_sizes(53, 80_000)] {
        let mut dec = GzipDecoder::new(ChunkedReader::new(&gz, sizes));
        let mut out = Vec::new();
        dec.read_to_end(&mut out).unwrap();
        dec.finish().unwrap();
        assert!(out == expected, "chunked gzip decode differs");
    }
    // Truncated member under 1-byte chunks errors instead of hanging.
    let cut = &gz[..gz.len() - 20];
    let mut dec = GzipDecoder::new(ChunkedReader::new(cut, vec![1]));
    assert!(dec.read_to_end(&mut Vec::new()).is_err());
}

#[test]
fn gzip_header_string_fields_are_bounded() {
    // FNAME without terminator far beyond the cap.
    let mut member = vec![0x1F, 0x8B, 0x08, 0x08, 0, 0, 0, 0, 0, 0x03];
    member.extend(std::iter::repeat_n(b'a', gzip::MAX_HEADER_FIELD_BYTES + 10));
    member.push(0);
    member.extend_from_slice(&deflate::deflate(b"x", CompressionLevel::Normal));
    member.extend_from_slice(&crc32(b"x").to_le_bytes());
    member.extend_from_slice(&1u32.to_le_bytes());
    let mut slice = &member[..];
    assert!(matches!(
        GzipHeader::read_from(&mut slice),
        Err(GzipError::HeaderTooLarge)
    ));
    assert!(matches!(
        gzip::decompress(&member),
        Err(GzipError::HeaderTooLarge)
    ));
    let mut dec = GzipDecoder::new(ChunkedReader::new(&member, vec![997]));
    assert!(dec.read_to_end(&mut Vec::new()).is_err());
    // FCOMMENT likewise; a field exactly at the cap is accepted.
    let mut member = vec![0x1F, 0x8B, 0x08, 0x10, 0, 0, 0, 0, 0, 0x03];
    member.extend(std::iter::repeat_n(b'c', gzip::MAX_HEADER_FIELD_BYTES));
    member.push(0);
    member.extend_from_slice(&deflate::deflate(b"x", CompressionLevel::Normal));
    member.extend_from_slice(&crc32(b"x").to_le_bytes());
    member.extend_from_slice(&1u32.to_le_bytes());
    assert_eq!(gzip::decompress(&member).unwrap(), b"x");
    let mut slice = &member[..];
    let h = GzipHeader::read_from(&mut slice).unwrap();
    assert_eq!(h.comment.unwrap().len(), gzip::MAX_HEADER_FIELD_BYTES);
    // Latin-1 header strings round-trip.
    let gz = gzip::compress_with(b"y", &GzipOptions::new().filename("caf\u{e9}"));
    let mut slice = &gz[..];
    let h = GzipHeader::read_from(&mut slice).unwrap();
    assert_eq!(h.filename.as_deref(), Some("caf\u{e9}"));
}

#[test]
fn inflate_decoder_empty_read_is_noop() {
    // Finished stream followed by trailing bytes: an empty read used to spin.
    let stream = [
        deflate::deflate(b"x", CompressionLevel::Normal),
        b"tail".to_vec(),
    ]
    .concat();
    let mut dec = InflateDecoder::new(&stream[..], 1024);
    assert_eq!(dec.read(&mut []).unwrap(), 0);
    let mut out = Vec::new();
    let mut buf = [0u8; 16];
    loop {
        assert_eq!(dec.read(&mut []).unwrap(), 0);
        match dec.read(&mut buf).unwrap() {
            0 => break,
            n => out.extend_from_slice(&buf[..n]),
        }
    }
    assert_eq!(out, b"x");
    assert_eq!(dec.read(&mut []).unwrap(), 0);

    // Output larger than the ring and input buffer: repeated empty reads
    // must not fill the buffers and fail with a spurious InvalidData.
    let data = lcg_bytes(300 * 1024);
    let packed = deflate::deflate(&data, CompressionLevel::Fast);
    let mut dec = InflateDecoder::new(&packed[..], data.len() as u64);
    for _ in 0..64 {
        assert_eq!(dec.read(&mut []).unwrap(), 0);
    }
    let mut out = Vec::new();
    dec.read_to_end(&mut out).unwrap();
    assert_eq!(out, data);
    dec.finish().unwrap();
}

#[test]
fn zip_extract_streams_and_removes_bad_files() {
    let len = 8usize << 20;
    let zeros = vec![0u8; len];
    let packed = deflate::deflate(&zeros, CompressionLevel::Normal);
    assert!(packed.len() < len / 64);

    // (a) Valid large Deflate entry streams to disk intact.
    let mut bytes = raw_zip("big.bin", &packed, 8);
    let cd = cd_offset(&bytes);
    bytes[cd + 16..cd + 20].copy_from_slice(&crc32(&zeros).to_le_bytes());
    bytes[cd + 24..cd + 28].copy_from_slice(&(len as u32).to_le_bytes());
    let dir = TempDir::new().unwrap();
    let mut r = ZipReader::new(std::io::Cursor::new(bytes.clone())).unwrap();
    assert_eq!(
        r.safe_extract_to(dir.path(), &ExtractOptions::new())
            .unwrap(),
        1
    );
    let got = std::fs::read(dir.path().join("big.bin")).unwrap();
    assert_eq!(got.len(), len);
    assert!(got.iter().all(|&b| b == 0));

    // (b) CRC mismatch: error and the partial file is removed.
    let mut bad = bytes.clone();
    bad[cd + 16] ^= 0xFF;
    let dir = TempDir::new().unwrap();
    let mut r = ZipReader::new(std::io::Cursor::new(bad)).unwrap();
    assert!(matches!(
        r.safe_extract_to(dir.path(), &ExtractOptions::new()),
        Err(ZipError::CrcMismatch { .. })
    ));
    assert!(!dir.path().join("big.bin").exists());

    // (c) Declared ~4 GiB under the default 4 GiB cap, but the stream holds
    // only 8 MiB: streamed (no huge allocation), SizeMismatch, file removed.
    let mut huge = bytes.clone();
    huge[cd + 24..cd + 28].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
    let dir = TempDir::new().unwrap();
    let mut r = ZipReader::new(std::io::Cursor::new(huge)).unwrap();
    assert!(matches!(
        r.safe_extract_to(dir.path(), &ExtractOptions::new()),
        Err(ZipError::SizeMismatch { expected: 0xFFFF_FFF0, actual }) if actual == len as u64
    ));
    assert!(!dir.path().join("big.bin").exists());

    // (d) Stream longer than declared: capped, typed Deflate error, removed.
    let mut short = bytes.clone();
    short[cd + 24..cd + 28].copy_from_slice(&1000u32.to_le_bytes());
    let dir = TempDir::new().unwrap();
    let mut r = ZipReader::new(std::io::Cursor::new(short)).unwrap();
    assert!(matches!(
        r.safe_extract_to(dir.path(), &ExtractOptions::new()),
        Err(ZipError::Deflate(DeflateError::OutputLimitExceeded))
    ));
    assert!(!dir.path().join("big.bin").exists());

    // (e) Without overwrite, a pre-existing file is refused and kept.
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("big.bin"), b"keep").unwrap();
    let mut r = ZipReader::new(std::io::Cursor::new(bytes)).unwrap();
    assert!(r
        .safe_extract_to(dir.path(), &ExtractOptions::new())
        .is_err());
    assert_eq!(std::fs::read(dir.path().join("big.bin")).unwrap(), b"keep");
}

#[test]
fn tar_pax_length_must_be_plain_digits() {
    for payload in ["+11 path=a\n", " 11 path=a\n", " path=a\n", "-1 path=a\n"] {
        let mut archive = tar_entry_bytes("pax", payload.as_bytes(), b'x', None);
        archive.extend(tar_entry_bytes("f.txt", b"hi", b'0', None));
        archive.extend_from_slice(&[0u8; 1024]);
        let mut r = TarReader::new(&archive[..]);
        let res = r.next_entry().map(|e| e.map(|e| e.path().to_string()));
        assert!(
            matches!(res, Err(TarError::InvalidPaxRecord(_))),
            "PAX payload {payload:?} gave {res:?}"
        );
    }
    // A plain decimal length still parses.
    let mut archive = tar_entry_bytes("pax", b"10 path=a\n", b'x', None);
    archive.extend(tar_entry_bytes("f.txt", b"hi", b'0', None));
    archive.extend_from_slice(&[0u8; 1024]);
    let mut r = TarReader::new(&archive[..]);
    assert_eq!(r.next_entry().unwrap().unwrap().path(), "a");
}
