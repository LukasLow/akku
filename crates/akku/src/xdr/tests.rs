//! Public-API tests for `akku::xdr`. Seams are listed in `_dev/DESIGN.md`.

use akku::xdr::{
    self, Decoder, Encoder, Error, FixedOpaque, Limits, Opaque, Result, XdrDecode, XdrEncode,
};

/// Deterministic xorshift64 PRNG so property-style round-trip loops need no
/// external crates.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    fn next_bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| (self.next_u64() >> 56) as u8).collect()
    }
}

/// Builds a wire-encoded variable-length opaque/string payload independently of
/// `Encoder` so decoder tests are never tautological.
fn raw_wire_var_bytes(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let len = u32::try_from(payload.len()).expect("test payload fits in u32");
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(payload);
    let pad = (4 - (payload.len() % 4)) % 4;
    out.extend(std::iter::repeat_n(0u8, pad));
    out
}

/// Builds a wire-encoded fixed-length opaque payload independently of `Encoder`.
fn raw_wire_fixed_bytes(payload: &[u8]) -> Vec<u8> {
    let mut out = payload.to_vec();
    let pad = (4 - (payload.len() % 4)) % 4;
    out.extend(std::iter::repeat_n(0u8, pad));
    out
}

// --- Alignment helpers -----------------------------------------------------

#[test]
fn padding_len_aligns_lengths_0_through_8_and_max_boundaries() {
    let expected = [
        (0usize, 0usize),
        (1, 3),
        (2, 2),
        (3, 1),
        (4, 0),
        (5, 3),
        (6, 2),
        (7, 1),
        (8, 0),
    ];
    for (len, pad) in expected {
        assert_eq!(xdr::padding_len(len), pad, "len={len}");
    }

    // u32::MAX == 0xffff_ffff == 4 * k + 3 -> needs 1 pad byte.
    let u32_max = u32::MAX as usize;
    assert_eq!(xdr::padding_len(u32_max - 3), 0);
    assert_eq!(xdr::padding_len(u32_max - 2), 3);
    assert_eq!(xdr::padding_len(u32_max - 1), 2);
    assert_eq!(xdr::padding_len(u32_max), 1);

    // usize::MAX is always 2^N - 1 == 4 * k + 3 on 32-bit and 64-bit targets.
    assert_eq!(xdr::padding_len(usize::MAX - 3), 0);
    assert_eq!(xdr::padding_len(usize::MAX - 2), 3);
    assert_eq!(xdr::padding_len(usize::MAX - 1), 2);
    assert_eq!(xdr::padding_len(usize::MAX), 1);
}

#[test]
fn padded_len_rounds_up_and_returns_none_on_usize_overflow() {
    let expected = [
        (0usize, Some(0usize)),
        (1, Some(4)),
        (2, Some(4)),
        (3, Some(4)),
        (4, Some(4)),
        (5, Some(8)),
        (6, Some(8)),
        (7, Some(8)),
        (8, Some(8)),
    ];
    for (len, padded) in expected {
        assert_eq!(xdr::padded_len(len), padded, "len={len}");
    }

    assert_eq!(xdr::padded_len(usize::MAX - 3), Some(usize::MAX - 3));
    assert_eq!(xdr::padded_len(usize::MAX - 2), None);
    assert_eq!(xdr::padded_len(usize::MAX - 1), None);
    assert_eq!(xdr::padded_len(usize::MAX), None);
}

// --- Limits defaults and builders ------------------------------------------

#[test]
fn limits_defaults_match_design_spec() {
    let expected = Limits {
        max_opaque_len: 16 * 1024 * 1024,
        max_array_len: 65_536,
        max_total_bytes: 64 * 1024 * 1024,
        max_depth: 64,
    };
    assert_eq!(Limits::new(), expected);
    assert_eq!(Limits::default(), expected);
}

#[test]
fn limits_builders_override_individual_fields() {
    let base = Limits::new();
    let custom = base
        .with_max_opaque_len(1024)
        .with_max_array_len(128)
        .with_max_total_bytes(8192)
        .with_max_depth(12);
    assert_eq!(custom.max_opaque_len, 1024);
    assert_eq!(custom.max_array_len, 128);
    assert_eq!(custom.max_total_bytes, 8192);
    assert_eq!(custom.max_depth, 12);
    assert_eq!(base.with_max_depth(9).max_opaque_len, base.max_opaque_len);
}

// --- RFC 4506 primitives (known-answer big-endian vectors) -----------------

#[test]
fn encodes_and_decodes_int_big_endian_twos_complement() {
    let vectors: &[(i32, [u8; 4])] = &[
        (0, [0x00, 0x00, 0x00, 0x00]),
        (1, [0x00, 0x00, 0x00, 0x01]),
        (-1, [0xff, 0xff, 0xff, 0xff]),
        (0x1234_5678, [0x12, 0x34, 0x56, 0x78]),
        (-2_147_483_647, [0x80, 0x00, 0x00, 0x01]),
        (i32::MAX, [0x7f, 0xff, 0xff, 0xff]),
        (i32::MIN, [0x80, 0x00, 0x00, 0x00]),
    ];

    for &(val, wire) in vectors {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.encode_int(val).unwrap();
        assert_eq!(enc.written(), 4);
        assert_eq!(buf.as_slice(), &wire);

        assert_eq!(xdr::encode(&val).unwrap(), wire);
        assert_eq!(xdr::decode::<i32>(&wire).unwrap(), val);

        let mut dec = Decoder::new(&wire);
        assert_eq!(dec.decode_int().unwrap(), val);
        assert_eq!(dec.position(), 4);
        assert_eq!(dec.remaining(), 0);
        assert!(dec.is_empty());
        dec.finish().unwrap();
    }
}

#[test]
fn encodes_and_decodes_uint_big_endian() {
    let vectors: &[(u32, [u8; 4])] = &[
        (0, [0x00, 0x00, 0x00, 0x00]),
        (1, [0x00, 0x00, 0x00, 0x01]),
        (0xdead_beef, [0xde, 0xad, 0xbe, 0xef]),
        (u32::MAX, [0xff, 0xff, 0xff, 0xff]),
    ];

    for &(val, wire) in vectors {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.encode_uint(val).unwrap();
        assert_eq!(buf.as_slice(), &wire);
        assert_eq!(xdr::encode(&val).unwrap(), wire);
        assert_eq!(xdr::decode::<u32>(&wire).unwrap(), val);

        let mut dec = Decoder::new(&wire);
        assert_eq!(dec.decode_uint().unwrap(), val);
        dec.finish().unwrap();
    }
}

#[test]
fn encodes_and_decodes_hyper_and_uhyper_big_endian() {
    let signed: &[(i64, [u8; 8])] = &[
        (0, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
        (1, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01]),
        (-1, [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]),
        (
            0x0123_4567_89ab_cdef,
            [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef],
        ),
        (i64::MAX, [0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]),
        (i64::MIN, [0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
    ];

    for &(val, wire) in signed {
        assert_eq!(xdr::encode(&val).unwrap(), wire);
        assert_eq!(xdr::decode::<i64>(&wire).unwrap(), val);

        let mut buf = Vec::new();
        Encoder::new(&mut buf).encode_hyper(val).unwrap();
        assert_eq!(buf.as_slice(), &wire);
        let mut dec = Decoder::new(&wire);
        assert_eq!(dec.decode_hyper().unwrap(), val);
        dec.finish().unwrap();
    }

    let unsigned: &[(u64, [u8; 8])] = &[
        (0, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
        (1, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01]),
        (
            0xfedc_ba98_7654_3210,
            [0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10],
        ),
        (u64::MAX, [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]),
    ];

    for &(val, wire) in unsigned {
        assert_eq!(xdr::encode(&val).unwrap(), wire);
        assert_eq!(xdr::decode::<u64>(&wire).unwrap(), val);

        let mut buf = Vec::new();
        Encoder::new(&mut buf).encode_uhyper(val).unwrap();
        assert_eq!(buf.as_slice(), &wire);
        let mut dec = Decoder::new(&wire);
        assert_eq!(dec.decode_uhyper().unwrap(), val);
        dec.finish().unwrap();
    }
}

#[test]
fn encodes_and_decodes_bool_strict_zero_and_one() {
    assert_eq!(xdr::encode(&false).unwrap(), [0, 0, 0, 0]);
    assert_eq!(xdr::encode(&true).unwrap(), [0, 0, 0, 1]);
    assert!(!xdr::decode::<bool>(&[0, 0, 0, 0]).unwrap());
    assert!(xdr::decode::<bool>(&[0, 0, 0, 1]).unwrap());

    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    enc.encode_bool(true).unwrap();
    enc.encode_bool(false).unwrap();
    assert_eq!(buf, [0, 0, 0, 1, 0, 0, 0, 0]);

    let mut dec = Decoder::new(&buf);
    assert!(dec.decode_bool().unwrap());
    assert!(!dec.decode_bool().unwrap());
    dec.finish().unwrap();
}

#[test]
fn encodes_and_decodes_ieee754_float_and_double_edge_values() {
    let f32_vectors: &[(f32, [u8; 4])] = &[
        (0.0, [0x00, 0x00, 0x00, 0x00]),
        (-0.0, [0x80, 0x00, 0x00, 0x00]),
        (1.0, [0x3f, 0x80, 0x00, 0x00]),
        (-1.5, [0xbf, 0xc0, 0x00, 0x00]),
        (f32::INFINITY, [0x7f, 0x80, 0x00, 0x00]),
        (f32::NEG_INFINITY, [0xff, 0x80, 0x00, 0x00]),
        (f32::MIN_POSITIVE, [0x00, 0x80, 0x00, 0x00]),
        (f32::from_bits(0x0000_0001), [0x00, 0x00, 0x00, 0x01]),
        (f32::from_bits(0x7fc0_1234), [0x7f, 0xc0, 0x12, 0x34]),
    ];

    for &(val, wire) in f32_vectors {
        let encoded = xdr::encode(&val).unwrap();
        assert_eq!(encoded.as_slice(), &wire);
        let decoded: f32 = xdr::decode(&wire).unwrap();
        assert_eq!(decoded.to_bits(), val.to_bits());

        let mut buf = Vec::new();
        Encoder::new(&mut buf).encode_float(val).unwrap();
        assert_eq!(buf.as_slice(), &wire);
        let mut dec = Decoder::new(&wire);
        assert_eq!(dec.decode_float().unwrap().to_bits(), val.to_bits());
        dec.finish().unwrap();
    }

    let f64_vectors: &[(f64, [u8; 8])] = &[
        (0.0, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
        (-0.0, [0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
        (1.0, [0x3f, 0xf0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
        (-1.5, [0xbf, 0xf8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]),
        (
            f64::INFINITY,
            [0x7f, 0xf0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        ),
        (
            f64::NEG_INFINITY,
            [0xff, 0xf0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        ),
        (
            f64::MIN_POSITIVE,
            [0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        ),
        (
            f64::from_bits(0x0000_0000_0000_0001),
            [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01],
        ),
        (
            f64::from_bits(0x7ff8_1234_5678_9abc),
            [0x7f, 0xf8, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc],
        ),
    ];

    for &(val, wire) in f64_vectors {
        let encoded = xdr::encode(&val).unwrap();
        assert_eq!(encoded.as_slice(), &wire);
        let decoded: f64 = xdr::decode(&wire).unwrap();
        assert_eq!(decoded.to_bits(), val.to_bits());

        let mut buf = Vec::new();
        Encoder::new(&mut buf).encode_double(val).unwrap();
        assert_eq!(buf.as_slice(), &wire);
        let mut dec = Decoder::new(&wire);
        assert_eq!(dec.decode_double().unwrap().to_bits(), val.to_bits());
        dec.finish().unwrap();
    }
}

#[test]
fn encodes_and_decodes_void_as_zero_bytes() {
    assert_eq!(xdr::encode(&()).unwrap(), Vec::<u8>::new());
    assert_eq!(xdr::decode::<()>(b""), Ok(()));

    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    enc.encode_void().unwrap();
    assert_eq!(enc.written(), 0);
    assert!(buf.is_empty());

    let mut dec = Decoder::new(&[0, 0, 0, 7]);
    dec.decode_void().unwrap();
    assert_eq!(dec.position(), 0);
    assert_eq!(dec.decode_uint().unwrap(), 7);
    dec.finish().unwrap();
}

#[test]
fn encodes_and_decodes_sparse_and_negative_enums() {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Status {
        Negative = -5,
        Ok = 0,
        Jukebox = 10008,
    }

    fn from_i32(v: i32) -> Option<Status> {
        match v {
            -5 => Some(Status::Negative),
            0 => Some(Status::Ok),
            10008 => Some(Status::Jukebox),
            _ => None,
        }
    }

    let cases = [
        (Status::Negative, [0xff, 0xff, 0xff, 0xfb]),
        (Status::Ok, [0x00, 0x00, 0x00, 0x00]),
        (Status::Jukebox, [0x00, 0x00, 0x27, 0x18]),
    ];

    for (status, wire) in cases {
        let mut buf = Vec::new();
        Encoder::new(&mut buf).encode_enum(status as i32).unwrap();
        assert_eq!(buf.as_slice(), &wire);

        let mut dec = Decoder::new(&wire);
        assert_eq!(dec.decode_enum(from_i32).unwrap(), status);
        dec.finish().unwrap();
    }
}

// --- Strict bool, enum, and discriminant rejection -------------------------

#[test]
fn rejects_invalid_bool_values_with_exact_offset() {
    let invalid_values: &[i32] = &[2, -1, i32::MIN, i32::MAX, 0x0001_0000, 0x0100_0000];

    for &bad in invalid_values {
        let wire = bad.to_be_bytes();
        assert_eq!(
            xdr::decode::<bool>(&wire),
            Err(Error::InvalidBool {
                offset: 0,
                value: bad,
            })
        );

        // Non-zero offset after a 4-byte prefix.
        let mut prefixed = vec![0, 0, 0, 9];
        prefixed.extend_from_slice(&wire);
        let mut dec = Decoder::new(&prefixed);
        assert_eq!(dec.decode_uint().unwrap(), 9);
        assert_eq!(
            dec.decode_bool(),
            Err(Error::InvalidBool {
                offset: 4,
                value: bad,
            })
        );

        // Option discriminant must also reject non-0/1 words.
        let mut dec = Decoder::new(&wire);
        assert_eq!(
            dec.decode_option(|d| d.decode_uint()),
            Err(Error::InvalidBool {
                offset: 0,
                value: bad,
            })
        );

        // Linked-list continuation flag must also reject non-0/1 words.
        let mut dec = Decoder::new(&wire);
        assert_eq!(
            dec.decode_list(None, |d| d.decode_uint()),
            Err(Error::InvalidBool {
                offset: 0,
                value: bad,
            })
        );
    }
}

#[test]
fn rejects_unknown_enum_value_with_exact_offset() {
    let wire = 99i32.to_be_bytes();
    let mut dec = Decoder::new(&wire);
    assert_eq!(
        dec.decode_enum(|v| (v == 1).then_some(())),
        Err(Error::InvalidEnum {
            offset: 0,
            value: 99,
        })
    );

    let mut prefixed = Vec::new();
    prefixed.extend_from_slice(&42u32.to_be_bytes());
    prefixed.extend_from_slice(&(-7i32).to_be_bytes());
    let mut dec = Decoder::new(&prefixed);
    assert_eq!(dec.decode_uint().unwrap(), 42);
    assert_eq!(
        dec.decode_enum(|v| (v == 0).then_some(())),
        Err(Error::InvalidEnum {
            offset: 4,
            value: -7,
        })
    );
}

#[test]
fn encodes_and_decodes_discriminated_unions_i32_and_u32_and_rejects_unknown() {
    #[derive(Debug, PartialEq, Eq)]
    enum SignedUnion {
        VoidArm,
        NegArm(u32),
    }

    // Signed union: void arm (0) and negative discriminant (-2) with payload.
    let mut buf = Vec::new();
    Encoder::new(&mut buf)
        .encode_union_i32(0, |e| e.encode_void())
        .unwrap();
    assert_eq!(buf, [0, 0, 0, 0]);

    let mut dec = Decoder::new(&buf);
    let parsed = dec
        .decode_union_i32(|tag, d| match tag {
            0 => {
                d.decode_void()?;
                Ok(SignedUnion::VoidArm)
            }
            -2 => Ok(SignedUnion::NegArm(d.decode_uint()?)),
            other => Err(Error::InvalidDiscriminant {
                offset: d.position().saturating_sub(4),
                value: other,
            }),
        })
        .unwrap();
    assert_eq!(parsed, SignedUnion::VoidArm);
    dec.finish().unwrap();

    buf.clear();
    Encoder::new(&mut buf)
        .encode_union_i32(-2, |e| e.encode_uint(0xabcd))
        .unwrap();
    assert_eq!(buf, [0xff, 0xff, 0xff, 0xfe, 0x00, 0x00, 0xab, 0xcd]);

    let mut dec = Decoder::new(&buf);
    let parsed = dec
        .decode_union_i32(|tag, d| match tag {
            0 => Ok(SignedUnion::VoidArm),
            -2 => Ok(SignedUnion::NegArm(d.decode_uint()?)),
            other => Err(Error::InvalidDiscriminant {
                offset: d.position().saturating_sub(4),
                value: other,
            }),
        })
        .unwrap();
    assert_eq!(parsed, SignedUnion::NegArm(0xabcd));
    dec.finish().unwrap();

    // Unknown signed discriminant -> InvalidDiscriminant with exact offset and value.
    let bad_signed = [0, 0, 0, 1, 0x00, 0x00, 0x00, 0x77];
    let mut dec = Decoder::new(&bad_signed);
    assert_eq!(dec.decode_uint().unwrap(), 1);
    let err = dec
        .decode_union_i32::<()>(|tag, d| {
            Err(Error::InvalidDiscriminant {
                offset: d.position().saturating_sub(4),
                value: tag,
            })
        })
        .unwrap_err();
    assert_eq!(
        err,
        Error::InvalidDiscriminant {
            offset: 4,
            value: 0x77,
        }
    );

    // Unsigned union with high-bit tag 0x8000_0001.
    let tag_hi: u32 = 0x8000_0001;
    buf.clear();
    Encoder::new(&mut buf)
        .encode_union_u32(tag_hi, |e| e.encode_int(-42))
        .unwrap();
    assert_eq!(buf, [0x80, 0x00, 0x00, 0x01, 0xff, 0xff, 0xff, 0xd6]);

    let mut dec = Decoder::new(&buf);
    let decoded = dec
        .decode_union_u32(|tag, d| match tag {
            0x8000_0001 => d.decode_int(),
            other => Err(Error::InvalidDiscriminant {
                offset: d.position().saturating_sub(4),
                value: other as i32,
            }),
        })
        .unwrap();
    assert_eq!(decoded, -42);
    dec.finish().unwrap();

    // Unknown unsigned discriminant reported by bit pattern cast to i32.
    let mut dec = Decoder::new(&[0x80, 0x00, 0x00, 0x05]);
    let err = dec
        .decode_union_u32::<()>(|tag, d| {
            Err(Error::InvalidDiscriminant {
                offset: d.position().saturating_sub(4),
                value: tag as i32,
            })
        })
        .unwrap_err();
    assert_eq!(
        err,
        Error::InvalidDiscriminant {
            offset: 0,
            value: 0x8000_0005u32 as i32,
        }
    );
}

// --- Fixed opaque, variable opaque, and UTF-8 strings ----------------------

#[test]
fn encodes_and_decodes_fixed_opaque_with_4_byte_padding() {
    let cases: &[(&[u8], &[u8])] = &[
        (b"", b""),
        (b"\xaa", b"\xaa\x00\x00\x00"),
        (b"\xaa\xbb", b"\xaa\xbb\x00\x00"),
        (b"\xaa\xbb\xcc", b"\xaa\xbb\xcc\x00"),
        (b"\xaa\xbb\xcc\xdd", b"\xaa\xbb\xcc\xdd"),
        (b"\x01\x02\x03\x04\x05", b"\x01\x02\x03\x04\x05\x00\x00\x00"),
        (
            b"\x01\x02\x03\x04\x05\x06\x07",
            b"\x01\x02\x03\x04\x05\x06\x07\x00",
        ),
        (
            b"\x01\x02\x03\x04\x05\x06\x07\x08",
            b"\x01\x02\x03\x04\x05\x06\x07\x08",
        ),
    ];

    for &(plain, expected_wire) in cases {
        let mut buf = Vec::new();
        Encoder::new(&mut buf).encode_fixed_opaque(plain).unwrap();
        assert_eq!(buf.as_slice(), expected_wire, "len={}", plain.len());

        // Zero-copy slice borrow points into `expected_wire`.
        let mut dec = Decoder::new(expected_wire);
        let borrowed = dec.decode_fixed_opaque_slice(plain.len()).unwrap();
        assert_eq!(borrowed, plain);
        if !plain.is_empty() {
            assert_eq!(borrowed.as_ptr(), expected_wire.as_ptr());
        }
        dec.finish().unwrap();

        // decode_fixed_opaque_into
        let mut dst = vec![0x99u8; plain.len()];
        let mut dec = Decoder::new(expected_wire);
        dec.decode_fixed_opaque_into(&mut dst).unwrap();
        assert_eq!(dst.as_slice(), plain);
        dec.finish().unwrap();
    }

    // Const-generic FixedOpaque<N> and decode_fixed_opaque::<N>.
    let v5 = FixedOpaque([1, 2, 3, 4, 5]);
    let w5 = xdr::encode(&v5).unwrap();
    assert_eq!(w5, [1, 2, 3, 4, 5, 0, 0, 0]);
    assert_eq!(xdr::decode::<FixedOpaque<5>>(&w5).unwrap(), v5);
    let mut dec = Decoder::new(&w5);
    assert_eq!(dec.decode_fixed_opaque::<5>().unwrap(), [1, 2, 3, 4, 5]);
    dec.finish().unwrap();

    let v0 = FixedOpaque([]);
    assert_eq!(xdr::encode(&v0).unwrap(), Vec::<u8>::new());
    assert_eq!(xdr::decode::<FixedOpaque<0>>(b"").unwrap(), v0);
}

#[test]
fn encodes_and_decodes_variable_opaque_with_length_and_padding() {
    for len in 0usize..=9 {
        let payload: Vec<u8> = (0..len).map(|i| (i as u8) + 0x11).collect();
        let expected_wire = raw_wire_var_bytes(&payload);

        let mut buf = Vec::new();
        Encoder::new(&mut buf).encode_opaque(&payload).unwrap();
        assert_eq!(buf, expected_wire, "len={len}");

        assert_eq!(
            xdr::encode(&Opaque(payload.clone())).unwrap(),
            expected_wire
        );
        assert_eq!(
            xdr::decode::<Opaque>(&expected_wire).unwrap(),
            Opaque(payload.clone())
        );

        let mut dec = Decoder::new(&expected_wire);
        let slice = dec.decode_opaque_slice(None).unwrap();
        assert_eq!(slice, payload.as_slice());
        if !payload.is_empty() {
            assert_eq!(slice.as_ptr(), expected_wire[4..].as_ptr());
        }
        dec.finish().unwrap();

        let mut dec = Decoder::new(&expected_wire);
        assert_eq!(dec.decode_opaque(Some(9)).unwrap(), payload);
        dec.finish().unwrap();
    }
}

#[test]
fn encodes_and_decodes_utf8_strings_including_multibyte_nul_and_crlf() {
    let cases: &[(&str, &[u8])] = &[
        ("", b"\x00\x00\x00\x00"),
        ("a", b"\x00\x00\x00\x01a\x00\x00\x00"),
        ("ab", b"\x00\x00\x00\x02ab\x00\x00"),
        ("abc", b"\x00\x00\x00\x03abc\x00"),
        ("abcd", b"\x00\x00\x00\x04abcd"),
        ("hello", b"\x00\x00\x00\x05hello\x00\x00\x00"),
        ("a\0b\r\nc", b"\x00\x00\x00\x06a\x00b\r\nc\x00\x00"),
        // 'é' is C3 A9 (2 bytes), '🦀' is F0 9F A6 80 (4 bytes) -> total 7 bytes, 1 pad byte.
        ("aé🦀", b"\x00\x00\x00\x07a\xc3\xa9\xf0\x9f\xa6\x80\x00"),
    ];

    for &(s, expected_wire) in cases {
        let mut buf = Vec::new();
        Encoder::new(&mut buf).encode_str(s).unwrap();
        assert_eq!(buf.as_slice(), expected_wire, "s={s:?}");

        assert_eq!(xdr::encode(s).unwrap(), expected_wire);
        assert_eq!(xdr::encode(&s.to_owned()).unwrap(), expected_wire);
        assert_eq!(xdr::decode::<String>(expected_wire).unwrap(), s);

        let mut dec = Decoder::new(expected_wire);
        let borrowed = dec.decode_str(None).unwrap();
        assert_eq!(borrowed, s);
        dec.finish().unwrap();

        let mut dec = Decoder::new(expected_wire);
        assert_eq!(dec.decode_string(None).unwrap(), s);
        dec.finish().unwrap();
    }
}

#[test]
fn rejects_nonzero_padding_at_every_pad_byte_position() {
    let pad_bytes = [0x01u8, 0x80, 0xff];

    for len in [1usize, 2, 3, 5, 6, 7] {
        let payload = vec![b'x'; len];
        let pad_count = (4 - (len % 4)) % 4;
        assert!(pad_count > 0);

        for pad_idx in 0..pad_count {
            for &bad_byte in &pad_bytes {
                // Fixed opaque: payload at 0..len, pad at len..len+pad_count.
                let mut fixed_wire = raw_wire_fixed_bytes(&payload);
                let fixed_off = len + pad_idx;
                fixed_wire[fixed_off] = bad_byte;

                let expected_fixed = Error::NonZeroPadding {
                    offset: fixed_off,
                    byte: bad_byte,
                };

                assert_eq!(
                    Decoder::new(&fixed_wire).decode_fixed_opaque_slice(len),
                    Err(expected_fixed),
                    "fixed_slice len={len} pad_idx={pad_idx} byte=0x{bad_byte:02x}"
                );

                let mut dst = vec![0u8; len];
                assert_eq!(
                    Decoder::new(&fixed_wire).decode_fixed_opaque_into(&mut dst),
                    Err(expected_fixed),
                    "fixed_into len={len} pad_idx={pad_idx} byte=0x{bad_byte:02x}"
                );

                match len {
                    1 => assert_eq!(
                        Decoder::new(&fixed_wire).decode_fixed_opaque::<1>(),
                        Err(expected_fixed)
                    ),
                    2 => assert_eq!(
                        Decoder::new(&fixed_wire).decode_fixed_opaque::<2>(),
                        Err(expected_fixed)
                    ),
                    3 => assert_eq!(
                        Decoder::new(&fixed_wire).decode_fixed_opaque::<3>(),
                        Err(expected_fixed)
                    ),
                    5 => assert_eq!(
                        Decoder::new(&fixed_wire).decode_fixed_opaque::<5>(),
                        Err(expected_fixed)
                    ),
                    _ => {}
                }

                // Variable opaque & string: 4-byte length prefix at 0..4, payload at 4..4+len.
                let mut var_wire = raw_wire_var_bytes(&payload);
                let var_off = 4 + len + pad_idx;
                var_wire[var_off] = bad_byte;

                let expected_var = Error::NonZeroPadding {
                    offset: var_off,
                    byte: bad_byte,
                };

                assert_eq!(
                    Decoder::new(&var_wire).decode_opaque_slice(None),
                    Err(expected_var),
                    "opaque_slice len={len} pad_idx={pad_idx} byte=0x{bad_byte:02x}"
                );
                assert_eq!(
                    Decoder::new(&var_wire).decode_opaque(None),
                    Err(expected_var),
                    "opaque len={len} pad_idx={pad_idx} byte=0x{bad_byte:02x}"
                );
                assert_eq!(
                    Decoder::new(&var_wire).decode_str(None),
                    Err(expected_var),
                    "str len={len} pad_idx={pad_idx} byte=0x{bad_byte:02x}"
                );
                assert_eq!(
                    Decoder::new(&var_wire).decode_string(None),
                    Err(expected_var),
                    "string len={len} pad_idx={pad_idx} byte=0x{bad_byte:02x}"
                );
            }
        }
    }
}

#[test]
fn rejects_invalid_utf8_sequences_in_strings_but_allows_them_in_opaque() {
    let invalid_cases: &[(&[u8], usize)] = &[
        // Lone continuation byte.
        (&[0x80], 0),
        // Valid ASCII prefix followed by invalid byte.
        (b"ab\xff", 2),
        // Overlong 2-byte encoding of NUL.
        (&[0xc0, 0x80], 0),
        // UTF-16 high surrogate U+D800.
        (&[0xed, 0xa0, 0x80], 0),
        // Truncated 3-byte sequence (euro sign missing 3rd byte).
        (&[0xe2, 0x82], 0),
    ];

    for &(raw, valid_up_to) in invalid_cases {
        let wire = raw_wire_var_bytes(raw);
        let expected = Error::InvalidUtf8 {
            offset: 4,
            valid_up_to,
        };

        assert_eq!(Decoder::new(&wire).decode_str(None), Err(expected));
        assert_eq!(Decoder::new(&wire).decode_string(None), Err(expected));
        assert_eq!(xdr::decode::<String>(&wire), Err(expected));

        // Non-UTF-8 raw bytes (e.g. NFSv3 filenames) succeed via opaque.
        assert_eq!(Decoder::new(&wire).decode_opaque_slice(None).unwrap(), raw);
        assert_eq!(Decoder::new(&wire).decode_opaque(None).unwrap(), raw);
    }
}

#[test]
fn enforces_field_max_len_and_min_with_limits_on_encode_and_decode() {
    // Exact field bound succeeds; bound + 1 fails.
    let ok_bytes = [0x41u8; 8];
    let too_long = [0x41u8; 9];

    let mut buf = Vec::new();
    Encoder::new(&mut buf)
        .encode_opaque_bounded(&ok_bytes, 8)
        .unwrap();
    assert_eq!(buf, raw_wire_var_bytes(&ok_bytes));

    let mut buf = Vec::new();
    assert_eq!(
        Encoder::new(&mut buf).encode_opaque_bounded(&too_long, 8),
        Err(Error::LengthLimit {
            offset: 0,
            len: 9,
            limit: 8,
        })
    );
    assert!(buf.is_empty());

    let mut buf = Vec::new();
    Encoder::new(&mut buf)
        .encode_str_bounded("12345678", 8)
        .unwrap();
    assert_eq!(buf, raw_wire_var_bytes(b"12345678"));

    let mut buf = Vec::new();
    assert_eq!(
        Encoder::new(&mut buf).encode_str_bounded("123456789", 8),
        Err(Error::LengthLimit {
            offset: 0,
            len: 9,
            limit: 8,
        })
    );
    assert!(buf.is_empty());

    // Decode at exact field bound succeeds; field bound + 1 fails.
    let wire_ok = raw_wire_var_bytes(&ok_bytes);
    let wire_bad = raw_wire_var_bytes(&too_long);
    assert_eq!(
        Decoder::new(&wire_ok).decode_opaque_slice(Some(8)).unwrap(),
        &ok_bytes
    );
    assert_eq!(
        Decoder::new(&wire_bad).decode_opaque_slice(Some(8)),
        Err(Error::LengthLimit {
            offset: 0,
            len: 9,
            limit: 8,
        })
    );
    assert_eq!(
        Decoder::new(&wire_bad).decode_str(Some(8)),
        Err(Error::LengthLimit {
            offset: 0,
            len: 9,
            limit: 8,
        })
    );

    // min(field_bound, limits_bound): field bound cannot raise global Limits.
    let tight = Limits::new().with_max_opaque_len(4);
    assert_eq!(
        Decoder::with_limits(&wire_ok, tight).decode_opaque(Some(64)),
        Err(Error::LengthLimit {
            offset: 0,
            len: 8,
            limit: 4,
        })
    );
    let mut buf = Vec::new();
    assert_eq!(
        Encoder::with_limits(&mut buf, tight).encode_opaque_bounded(&ok_bytes, 64),
        Err(Error::LengthLimit {
            offset: 0,
            len: 8,
            limit: 4,
        })
    );
}

// --- Arrays, options, linked lists, Box, traits ----------------------------

#[test]
fn encodes_and_decodes_fixed_and_variable_arrays() {
    // Fixed array [u32; 3]
    let fixed = [1u32, 2, 3];
    let expected_fixed = [0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3];
    assert_eq!(xdr::encode(&fixed).unwrap(), expected_fixed);
    assert_eq!(xdr::decode::<[u32; 3]>(&expected_fixed).unwrap(), fixed);

    let mut buf = Vec::new();
    Encoder::new(&mut buf)
        .encode_fixed_array(&fixed, |e, &v| e.encode_uint(v))
        .unwrap();
    assert_eq!(buf, expected_fixed);

    let mut dec = Decoder::new(&expected_fixed);
    assert_eq!(
        dec.decode_fixed_array::<u32, 3>(|d| d.decode_uint())
            .unwrap(),
        fixed
    );
    dec.finish().unwrap();

    let mut dec = Decoder::new(&expected_fixed);
    assert_eq!(
        dec.decode_fixed_array_vec(3, |d| d.decode_uint()).unwrap(),
        vec![1, 2, 3]
    );
    dec.finish().unwrap();

    // Fixed array of padded elements verifies per-element 4-byte alignment.
    let fixed_opaques = [Opaque(vec![0xaa]), Opaque(vec![0xbb, 0xcc])];
    let expected_op = [
        0, 0, 0, 1, 0xaa, 0, 0, 0, // element 0
        0, 0, 0, 2, 0xbb, 0xcc, 0, 0, // element 1
    ];
    assert_eq!(xdr::encode(&fixed_opaques).unwrap(), expected_op);
    assert_eq!(
        xdr::decode::<[Opaque; 2]>(&expected_op).unwrap(),
        fixed_opaques
    );

    // Variable-length array (empty and populated)
    let empty: Vec<u32> = Vec::new();
    assert_eq!(xdr::encode(&empty).unwrap(), [0, 0, 0, 0]);
    assert_eq!(xdr::decode::<Vec<u32>>(&[0, 0, 0, 0]).unwrap(), empty);

    let items = vec![10i32, -20, 30];
    let expected_var = [
        0, 0, 0, 3, // count
        0, 0, 0, 10, // 10
        0xff, 0xff, 0xff, 0xec, // -20
        0, 0, 0, 30, // 30
    ];
    assert_eq!(xdr::encode(&items).unwrap(), expected_var);
    assert_eq!(xdr::encode(items.as_slice()).unwrap(), expected_var);
    assert_eq!(xdr::decode::<Vec<i32>>(&expected_var).unwrap(), items);

    // Bounded variable array: 3 passes max_count=3, fails max_count=2.
    let mut dec = Decoder::new(&expected_var);
    assert_eq!(
        dec.decode_array(Some(3), |d| d.decode_int()).unwrap(),
        items
    );
    dec.finish().unwrap();

    assert_eq!(
        Decoder::new(&expected_var).decode_array(Some(2), |d| d.decode_int()),
        Err(Error::LengthLimit {
            offset: 0,
            len: 3,
            limit: 2,
        })
    );

    let mut buf = Vec::new();
    assert_eq!(
        Encoder::new(&mut buf).encode_array(&items, Some(2), |e, &v| e.encode_int(v)),
        Err(Error::LengthLimit {
            offset: 0,
            len: 3,
            limit: 2,
        })
    );
    assert!(buf.is_empty());
}

#[test]
fn encodes_and_decodes_options_and_nested_options() {
    let none: Option<i32> = None;
    let some: Option<i32> = Some(42);
    assert_eq!(xdr::encode(&none).unwrap(), [0, 0, 0, 0]);
    assert_eq!(xdr::encode(&some).unwrap(), [0, 0, 0, 1, 0, 0, 0, 42]);
    assert_eq!(xdr::decode::<Option<i32>>(&[0, 0, 0, 0]).unwrap(), None);
    assert_eq!(
        xdr::decode::<Option<i32>>(&[0, 0, 0, 1, 0, 0, 0, 42]).unwrap(),
        Some(42)
    );

    let nested: Option<Option<u32>> = Some(Some(7));
    let wire = [0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 7];
    assert_eq!(xdr::encode(&nested).unwrap(), wire);
    assert_eq!(xdr::decode::<Option<Option<u32>>>(&wire).unwrap(), nested);

    let mut buf = Vec::new();
    Encoder::new(&mut buf)
        .encode_option(Some(&"ok"), |e, s| e.encode_str(s))
        .unwrap();
    assert_eq!(buf, [0, 0, 0, 1, 0, 0, 0, 2, b'o', b'k', 0, 0]);

    let mut dec = Decoder::new(&buf);
    assert_eq!(
        dec.decode_option(|d| d.decode_string(None)).unwrap(),
        Some(String::from("ok"))
    );
    dec.finish().unwrap();
}

#[test]
fn encodes_and_decodes_linked_lists_iteratively_and_handles_10000_items() {
    // Empty list is a single false (0) word.
    let mut buf = Vec::new();
    Encoder::new(&mut buf)
        .encode_list::<u32>(&[], None, |e, &v| e.encode_uint(v))
        .unwrap();
    assert_eq!(buf, [0, 0, 0, 0]);

    let mut dec = Decoder::new(&buf);
    assert_eq!(
        dec.decode_list(None, |d| d.decode_uint()).unwrap(),
        Vec::<u32>::new()
    );
    dec.finish().unwrap();

    // 3-element list: [1, item0, 1, item1, 1, item2, 0]
    let small = [10u32, 20, 30];
    let expected_small = [
        0, 0, 0, 1, 0, 0, 0, 10, //
        0, 0, 0, 1, 0, 0, 0, 20, //
        0, 0, 0, 1, 0, 0, 0, 30, //
        0, 0, 0, 0, // terminator
    ];
    buf.clear();
    Encoder::new(&mut buf)
        .encode_list(&small, Some(3), |e, &v| e.encode_uint(v))
        .unwrap();
    assert_eq!(buf, expected_small);

    let mut dec = Decoder::new(&expected_small);
    assert_eq!(
        dec.decode_list(Some(3), |d| d.decode_uint()).unwrap(),
        small
    );
    dec.finish().unwrap();

    // Field bound exceeded on list decode (4th element rejected before consuming).
    assert_eq!(
        Decoder::new(&expected_small).decode_list(Some(2), |d| d.decode_uint()),
        Err(Error::LengthLimit {
            offset: 16,
            len: 3,
            limit: 2,
        })
    );

    // 10,000-element linked list with default max_depth=64 proves O(1) stack
    // and O(1) depth usage.
    let big: Vec<u32> = (0u32..10_000).collect();
    let mut big_wire = Vec::new();
    Encoder::new(&mut big_wire)
        .encode_list(&big, None, |e, &v| e.encode_uint(v))
        .unwrap();
    assert_eq!(big_wire.len(), 10_000 * 8 + 4);

    let mut dec = Decoder::new(&big_wire);
    let decoded = dec.decode_list(None, |d| d.decode_uint()).unwrap();
    assert_eq!(decoded, big);
    dec.finish().unwrap();
}

#[test]
fn decodes_concatenated_messages_tracking_position_remaining_and_rest() {
    let mut wire = Vec::new();
    wire.extend_from_slice(&1u32.to_be_bytes());
    wire.extend_from_slice(&2u32.to_be_bytes());
    wire.extend_from_slice(&3u32.to_be_bytes());

    let mut dec = Decoder::new(&wire);
    assert_eq!(dec.position(), 0);
    assert_eq!(dec.remaining(), 12);
    assert!(!dec.is_empty());
    assert_eq!(dec.rest(), wire.as_slice());

    assert_eq!(dec.decode_uint().unwrap(), 1);
    assert_eq!(dec.position(), 4);
    assert_eq!(dec.remaining(), 8);
    assert_eq!(dec.rest(), &wire[4..]);

    assert_eq!(dec.decode_uint().unwrap(), 2);
    assert_eq!(dec.decode_uint().unwrap(), 3);
    assert_eq!(dec.position(), 12);
    assert_eq!(dec.remaining(), 0);
    assert!(dec.is_empty());
    assert_eq!(dec.rest(), &[] as &[u8]);
    dec.finish().unwrap();
}

#[test]
fn box_and_borrowed_slice_and_str_trait_impls_round_trip() {
    let boxed = Box::new(0x1234_5678i32);
    let wire = xdr::encode(&boxed).unwrap();
    assert_eq!(wire, [0x12, 0x34, 0x56, 0x78]);
    assert_eq!(xdr::decode::<Box<i32>>(&wire).unwrap(), boxed);

    // Unsized `str` and `&str`, unsized `[T]` and `&[T]`.
    let s = "hi";
    assert_eq!(xdr::encode(s).unwrap(), [0, 0, 0, 2, b'h', b'i', 0, 0]);
    assert_eq!(xdr::encode(&s).unwrap(), [0, 0, 0, 2, b'h', b'i', 0, 0]);

    let slice: &[u32] = &[5, 6];
    assert_eq!(
        xdr::encode(slice).unwrap(),
        [0, 0, 0, 2, 0, 0, 0, 5, 0, 0, 0, 6]
    );
    assert_eq!(
        xdr::encode(&slice).unwrap(),
        [0, 0, 0, 2, 0, 0, 0, 5, 0, 0, 0, 6]
    );
}

// --- RFC 4506 §7 & NFSv3-shaped composite vectors --------------------------

#[derive(Debug, PartialEq, Eq)]
enum RfcFileType {
    Text,
    Data(String),
    Exec(String),
}

#[derive(Debug, PartialEq, Eq)]
struct RfcFile {
    filename: String,
    filetype: RfcFileType,
    owner: String,
    data: Vec<u8>,
}

impl XdrEncode for RfcFile {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        enc.nest(|e| {
            e.encode_str_bounded(&self.filename, 255)?;
            match &self.filetype {
                RfcFileType::Text => e.encode_union_i32(0, |e| e.encode_void())?,
                RfcFileType::Data(creator) => {
                    e.encode_union_i32(1, |e| e.encode_str_bounded(creator, 255))?
                }
                RfcFileType::Exec(interp) => {
                    e.encode_union_i32(2, |e| e.encode_str_bounded(interp, 255))?
                }
            }
            e.encode_str_bounded(&self.owner, 32)?;
            e.encode_opaque_bounded(&self.data, 65_535)
        })
    }
}

impl XdrDecode for RfcFile {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.nest(|d| {
            let filename = d.decode_string(Some(255))?;
            let filetype = d.decode_union_i32(|tag, d| match tag {
                0 => {
                    d.decode_void()?;
                    Ok(RfcFileType::Text)
                }
                1 => Ok(RfcFileType::Data(d.decode_string(Some(255))?)),
                2 => Ok(RfcFileType::Exec(d.decode_string(Some(255))?)),
                other => Err(Error::InvalidDiscriminant {
                    offset: d.position().saturating_sub(4),
                    value: other,
                }),
            })?;
            let owner = d.decode_string(Some(32))?;
            let data = d.decode_opaque(Some(65_535))?;
            Ok(Self {
                filename,
                filetype,
                owner,
                data,
            })
        })
    }
}

#[test]
fn matches_rfc4506_section_7_file_example_byte_for_byte() {
    // Exact example from RFC 4506 §7:
    //   filename = "sillyprog", type = EXEC("lisp"), owner = "john", data = "(quit)"
    let expected_wire: &[u8] = &[
        // filename: len 9 + "sillyprog" + 3 zero pad bytes
        0x00, 0x00, 0x00, 0x09, //
        0x73, 0x69, 0x6c, 0x6c, // "sill"
        0x79, 0x70, 0x72, 0x6f, // "ypro"
        0x67, 0x00, 0x00, 0x00, // "g\0\0\0"
        // filetype: EXEC (2) + len 4 + "lisp"
        0x00, 0x00, 0x00, 0x02, //
        0x00, 0x00, 0x00, 0x04, //
        0x6c, 0x69, 0x73, 0x70, // "lisp"
        // owner: len 4 + "john"
        0x00, 0x00, 0x00, 0x04, //
        0x6a, 0x6f, 0x68, 0x6e, // "john"
        // data: len 6 + "(quit)" + 2 zero pad bytes
        0x00, 0x00, 0x00, 0x06, //
        0x28, 0x71, 0x75, 0x69, // "(qui"
        0x74, 0x29, 0x00, 0x00, // "t)\0\0"
    ];

    let file = RfcFile {
        filename: String::from("sillyprog"),
        filetype: RfcFileType::Exec(String::from("lisp")),
        owner: String::from("john"),
        data: b"(quit)".to_vec(),
    };

    let encoded = xdr::encode(&file).unwrap();
    assert_eq!(encoded.as_slice(), expected_wire);
    assert_eq!(xdr::decode::<RfcFile>(expected_wire).unwrap(), file);

    // Also verify the TEXT (void arm) and DATA union arms.
    for filetype in [RfcFileType::Text, RfcFileType::Data(String::from("gcc"))] {
        let sample = RfcFile {
            filename: String::from("a.txt"),
            filetype,
            owner: String::from("root"),
            data: b"abc".to_vec(),
        };
        let bytes = xdr::encode(&sample).unwrap();
        assert_eq!(bytes.len() % 4, 0);
        assert_eq!(xdr::decode::<RfcFile>(&bytes).unwrap(), sample);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ftype3 {
    Reg = 1,
    Dir = 2,
    Lnk = 5,
}

#[derive(Debug, PartialEq, Eq)]
struct Entry3 {
    fileid: u64,
    name: Vec<u8>,
    cookie: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum Readdir3Res {
    Ok {
        fh: Vec<u8>,
        ftype: Ftype3,
        size: u64,
        cookieverf: [u8; 8],
        entries: Vec<Entry3>,
        eof: bool,
    },
    Jukebox {
        post_op_mode: Option<u32>,
    },
}

impl XdrEncode for Readdir3Res {
    fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
        match self {
            Self::Ok {
                fh,
                ftype,
                size,
                cookieverf,
                entries,
                eof,
            } => enc.encode_union_i32(0, |e| {
                e.encode_opaque_bounded(fh, 64)?;
                e.encode_enum(*ftype as i32)?;
                e.encode_uhyper(*size)?;
                e.encode_fixed_opaque(cookieverf)?;
                e.encode_list(entries, None, |e, entry| {
                    e.nest(|e| {
                        e.encode_uhyper(entry.fileid)?;
                        e.encode_opaque(&entry.name)?;
                        e.encode_uhyper(entry.cookie)
                    })
                })?;
                e.encode_bool(*eof)
            }),
            Self::Jukebox { post_op_mode } => enc.encode_union_i32(10008, |e| {
                e.encode_option(post_op_mode.as_ref(), |e, &m| e.encode_uint(m))
            }),
        }
    }
}

impl XdrDecode for Readdir3Res {
    fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
        dec.decode_union_i32(|status, d| match status {
            0 => {
                let fh = d.decode_opaque(Some(64))?;
                let ftype = d.decode_enum(|v| match v {
                    1 => Some(Ftype3::Reg),
                    2 => Some(Ftype3::Dir),
                    5 => Some(Ftype3::Lnk),
                    _ => None,
                })?;
                let size = d.decode_uhyper()?;
                let cookieverf = d.decode_fixed_opaque::<8>()?;
                let entries = d.decode_list(None, |d| {
                    d.nest(|d| {
                        let fileid = d.decode_uhyper()?;
                        let name = d.decode_opaque(None)?;
                        let cookie = d.decode_uhyper()?;
                        Ok(Entry3 {
                            fileid,
                            name,
                            cookie,
                        })
                    })
                })?;
                let eof = d.decode_bool()?;
                Ok(Self::Ok {
                    fh,
                    ftype,
                    size,
                    cookieverf,
                    entries,
                    eof,
                })
            }
            10008 => {
                let post_op_mode = d.decode_option(|d| d.decode_uint())?;
                Ok(Self::Jukebox { post_op_mode })
            }
            other => Err(Error::InvalidDiscriminant {
                offset: d.position().saturating_sub(4),
                value: other,
            }),
        })
    }
}

#[test]
fn round_trips_nfsv3_shaped_composite_message() {
    let ok_msg = Readdir3Res::Ok {
        fh: vec![0x01, 0x02, 0x03, 0x04, 0x05],
        ftype: Ftype3::Dir,
        size: 4096,
        cookieverf: [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80],
        entries: vec![
            Entry3 {
                fileid: 1,
                name: b"readme.txt".to_vec(),
                cookie: 100,
            },
            Entry3 {
                fileid: 2,
                // Raw non-UTF-8 OS filename bytes carried via opaque.
                name: vec![0x66, 0x6f, 0xff, 0xfe],
                cookie: 200,
            },
        ],
        eof: true,
    };

    let wire = xdr::encode(&ok_msg).unwrap();
    assert_eq!(wire.len() % 4, 0);
    assert_eq!(xdr::decode::<Readdir3Res>(&wire).unwrap(), ok_msg);

    let err_msg = Readdir3Res::Jukebox {
        post_op_mode: Some(0o755),
    };
    let err_wire = xdr::encode(&err_msg).unwrap();
    assert_eq!(
        err_wire,
        [
            0x00, 0x00, 0x27, 0x18, // NFS3ERR_JUKEBOX = 10008
            0x00, 0x00, 0x00, 0x01, // Some
            0x00, 0x00, 0x01, 0xed, // 0o755 == 493 == 0x01ed
        ]
    );
    assert_eq!(xdr::decode::<Readdir3Res>(&err_wire).unwrap(), err_msg);
}

// --- Truncation and trailing bytes -----------------------------------------

#[test]
fn rejects_empty_and_short_inputs_for_all_primitive_decoders() {
    for len in 0usize..4 {
        let short = &b"\x00\x01\x02"[..len];
        let expected4 = Error::UnexpectedEof {
            offset: 0,
            needed: 4,
            remaining: len,
        };
        assert_eq!(Decoder::new(short).decode_int(), Err(expected4));
        assert_eq!(Decoder::new(short).decode_uint(), Err(expected4));
        assert_eq!(Decoder::new(short).decode_enum(Some), Err(expected4));
        assert_eq!(Decoder::new(short).decode_bool(), Err(expected4));
        assert_eq!(Decoder::new(short).decode_float(), Err(expected4));
    }

    for len in 0usize..8 {
        let short = &b"\x00\x01\x02\x03\x04\x05\x06"[..len];
        let expected8 = Error::UnexpectedEof {
            offset: 0,
            needed: 8,
            remaining: len,
        };
        assert_eq!(Decoder::new(short).decode_hyper(), Err(expected8));
        assert_eq!(Decoder::new(short).decode_uhyper(), Err(expected8));
        assert_eq!(Decoder::new(short).decode_double(), Err(expected8));
    }
}

#[test]
fn rejects_truncated_composite_message_at_every_byte_offset() {
    let msg = Readdir3Res::Ok {
        fh: vec![1, 2, 3, 4, 5],
        ftype: Ftype3::Lnk,
        size: 123_456,
        cookieverf: [1, 2, 3, 4, 5, 6, 7, 8],
        entries: vec![Entry3 {
            fileid: 9,
            name: b"abc".to_vec(),
            cookie: 10,
        }],
        eof: false,
    };

    let encoded = xdr::encode(&msg).unwrap();
    assert!(!encoded.is_empty());

    for cut in 0..encoded.len() {
        let err = xdr::decode::<Readdir3Res>(&encoded[..cut]).unwrap_err();
        match err {
            Error::UnexpectedEof {
                needed, remaining, ..
            } => {
                assert!(
                    remaining < needed,
                    "cut={cut}: expected remaining < needed, got remaining={remaining}, needed={needed}"
                );
            }
            other => panic!("cut={cut}: expected UnexpectedEof, got {other:?}"),
        }
    }

    // Also test RFC 4506 §7 struct file at every byte offset.
    let file = RfcFile {
        filename: String::from("sillyprog"),
        filetype: RfcFileType::Exec(String::from("lisp")),
        owner: String::from("john"),
        data: b"(quit)".to_vec(),
    };
    let file_wire = xdr::encode(&file).unwrap();
    for cut in 0..file_wire.len() {
        assert!(
            matches!(
                xdr::decode::<RfcFile>(&file_wire[..cut]),
                Err(Error::UnexpectedEof { .. })
            ),
            "RfcFile cut={cut} did not return UnexpectedEof"
        );
    }
}

#[test]
fn rejects_trailing_bytes_in_decode_decode_with_and_finish() {
    let one_extra = [0x00, 0x00, 0x00, 0x2a, 0x00];
    let four_extra = [0x00, 0x00, 0x00, 0x2a, 0x00, 0x00, 0x00, 0x00];

    assert_eq!(
        xdr::decode::<u32>(&one_extra),
        Err(Error::TrailingBytes {
            offset: 4,
            remaining: 1,
        })
    );
    assert_eq!(
        xdr::decode_with::<u32>(&four_extra, Limits::new()),
        Err(Error::TrailingBytes {
            offset: 4,
            remaining: 4,
        })
    );

    let mut dec = Decoder::new(&four_extra);
    assert_eq!(dec.decode_uint().unwrap(), 42);
    assert_eq!(
        dec.finish(),
        Err(Error::TrailingBytes {
            offset: 4,
            remaining: 4,
        })
    );
}

// --- DoS limits, u32::MAX prefixes, bombs, and depth counter balance -------

#[test]
fn enforces_max_opaque_len_at_exact_limit_and_one_past() {
    let limits = Limits::new().with_max_opaque_len(16);
    let ok_str = "a".repeat(16);
    let bad_str = "a".repeat(17);

    // Encode at exact limit succeeds, limit + 1 fails.
    let encoded_str = xdr::encode_with(&ok_str, limits).unwrap();
    assert_eq!(
        xdr::encode_with(&bad_str, limits),
        Err(Error::LengthLimit {
            offset: 0,
            len: 17,
            limit: 16,
        })
    );

    let ok_opaque = Opaque(vec![0x55; 16]);
    let bad_opaque = Opaque(vec![0x55; 17]);
    let encoded_opaque = xdr::encode_with(&ok_opaque, limits).unwrap();
    assert_eq!(
        xdr::encode_with(&bad_opaque, limits),
        Err(Error::LengthLimit {
            offset: 0,
            len: 17,
            limit: 16,
        })
    );

    // Decode at exact limit succeeds, limit + 1 fails.
    assert_eq!(
        xdr::decode_with::<String>(&encoded_str, limits).unwrap(),
        ok_str
    );
    assert_eq!(
        xdr::decode_with::<Opaque>(&encoded_opaque, limits).unwrap(),
        ok_opaque
    );

    let wire_bad_str = raw_wire_var_bytes(bad_str.as_bytes());
    assert_eq!(
        xdr::decode_with::<String>(&wire_bad_str, limits),
        Err(Error::LengthLimit {
            offset: 0,
            len: 17,
            limit: 16,
        })
    );
    assert_eq!(
        xdr::decode_with::<Opaque>(&wire_bad_str, limits),
        Err(Error::LengthLimit {
            offset: 0,
            len: 17,
            limit: 16,
        })
    );
}

#[test]
fn enforces_max_array_len_at_exact_limit_and_one_past() {
    let limits = Limits::new().with_max_array_len(8);
    let ok_items: Vec<u32> = (0..8).collect();
    let bad_items: Vec<u32> = (0..9).collect();

    // Variable array encode/decode.
    let encoded_ok = xdr::encode_with(&ok_items, limits).unwrap();
    assert_eq!(
        xdr::encode_with(&bad_items, limits),
        Err(Error::LengthLimit {
            offset: 0,
            len: 9,
            limit: 8,
        })
    );
    assert_eq!(
        xdr::decode_with::<Vec<u32>>(&encoded_ok, limits).unwrap(),
        ok_items
    );

    let encoded_bad = xdr::encode(&bad_items).unwrap();
    assert_eq!(
        xdr::decode_with::<Vec<u32>>(&encoded_bad, limits),
        Err(Error::LengthLimit {
            offset: 0,
            len: 9,
            limit: 8,
        })
    );

    // Linked list encode/decode.
    let mut list_ok = Vec::new();
    Encoder::with_limits(&mut list_ok, limits)
        .encode_list(&ok_items, None, |e, &v| e.encode_uint(v))
        .unwrap();
    assert_eq!(
        Decoder::with_limits(&list_ok, limits)
            .decode_list(None, |d| d.decode_uint())
            .unwrap(),
        ok_items
    );

    let mut list_bad_enc = Vec::new();
    assert_eq!(
        Encoder::with_limits(&mut list_bad_enc, limits)
            .encode_list(&bad_items, None, |e, &v| e.encode_uint(v)),
        Err(Error::LengthLimit {
            offset: 0,
            len: 9,
            limit: 8,
        })
    );
    assert!(list_bad_enc.is_empty());

    let mut list_bad_wire = Vec::new();
    Encoder::new(&mut list_bad_wire)
        .encode_list(&bad_items, None, |e, &v| e.encode_uint(v))
        .unwrap();
    assert_eq!(
        Decoder::with_limits(&list_bad_wire, limits).decode_list(None, |d| d.decode_uint()),
        Err(Error::LengthLimit {
            offset: 64,
            len: 9,
            limit: 8,
        })
    );
}

#[test]
fn enforces_max_total_bytes_at_exact_limit_and_one_past_on_encode_and_decode() {
    let limits = Limits::new().with_max_total_bytes(16);

    // 4 x u32 = 16 bytes -> exact limit succeeds.
    let ok = [1u32, 2, 3, 4];
    let encoded = xdr::encode_with(&ok, limits).unwrap();
    assert_eq!(encoded.len(), 16);
    assert_eq!(xdr::decode_with::<[u32; 4]>(&encoded, limits).unwrap(), ok);

    // 5 x u32 = 20 bytes -> exceeds limit on encode and decode.
    let bad = [1u32, 2, 3, 4, 5];
    assert_eq!(
        xdr::encode_with(&bad, limits),
        Err(Error::TotalBytesLimit {
            offset: 16,
            limit: 16,
        })
    );

    let bad_wire = xdr::encode(&bad).unwrap();
    assert_eq!(
        xdr::decode_with::<[u32; 5]>(&bad_wire, limits),
        Err(Error::TotalBytesLimit {
            offset: 16,
            limit: 16,
        })
    );
}

#[test]
fn enforces_max_depth_and_restores_depth_after_error() {
    fn nest_encode(enc: &mut Encoder<'_>, depth: usize) -> Result<()> {
        if depth == 0 {
            enc.encode_uint(1)
        } else {
            enc.nest(|e| nest_encode(e, depth - 1))
        }
    }

    fn nest_decode(dec: &mut Decoder<'_>, depth: usize) -> Result<u32> {
        if depth == 0 {
            dec.decode_uint()
        } else {
            dec.nest(|d| nest_decode(d, depth - 1))
        }
    }

    let limits = Limits::new().with_max_depth(4);

    // Exact depth 4 succeeds on encode and decode.
    let mut buf = Vec::new();
    nest_encode(&mut Encoder::with_limits(&mut buf, limits), 4).unwrap();
    assert_eq!(buf, [0, 0, 0, 1]);

    let mut dec = Decoder::with_limits(&buf, limits);
    assert_eq!(nest_decode(&mut dec, 4).unwrap(), 1);
    dec.finish().unwrap();

    // Depth 5 fails with DepthLimit on encode and decode.
    let mut buf = Vec::new();
    assert_eq!(
        nest_encode(&mut Encoder::with_limits(&mut buf, limits), 5),
        Err(Error::DepthLimit {
            offset: 0,
            limit: 4,
        })
    );
    assert!(buf.is_empty());

    let mut dec = Decoder::with_limits(&[0, 0, 0, 1], limits);
    assert_eq!(
        nest_decode(&mut dec, 5),
        Err(Error::DepthLimit {
            offset: 0,
            limit: 4,
        })
    );

    // Depth counter balance: after an inner error inside `nest`, depth must
    // return to its previous level so subsequent `nest` calls still work.
    let mut dec = Decoder::with_limits(&[0, 0, 0, 2, 0, 0, 0, 1], Limits::new().with_max_depth(1));
    assert_eq!(
        dec.nest(|d| d.decode_bool()),
        Err(Error::InvalidBool {
            offset: 0,
            value: 2,
        })
    );
    assert!(dec.nest(|d| d.decode_bool()).unwrap());
    dec.finish().unwrap();
}

#[test]
fn rejects_length_prefixes_near_u32_max_without_allocating() {
    let huge_lengths: &[u32] = &[u32::MAX - 3, u32::MAX - 2, u32::MAX - 1, u32::MAX];

    for &len in huge_lengths {
        let wire = len.to_be_bytes();

        // Default limits reject immediately with LengthLimit.
        assert_eq!(
            Decoder::new(&wire).decode_opaque(None),
            Err(Error::LengthLimit {
                offset: 0,
                len: len as usize,
                limit: 16 * 1024 * 1024,
            })
        );
        assert_eq!(
            Decoder::new(&wire).decode_opaque_slice(None),
            Err(Error::LengthLimit {
                offset: 0,
                len: len as usize,
                limit: 16 * 1024 * 1024,
            })
        );
        assert_eq!(
            Decoder::new(&wire).decode_str(None),
            Err(Error::LengthLimit {
                offset: 0,
                len: len as usize,
                limit: 16 * 1024 * 1024,
            })
        );
        assert_eq!(
            Decoder::new(&wire).decode_string(None),
            Err(Error::LengthLimit {
                offset: 0,
                len: len as usize,
                limit: 16 * 1024 * 1024,
            })
        );
        assert_eq!(
            Decoder::new(&wire).decode_array(None, |d| d.decode_uint()),
            Err(Error::LengthLimit {
                offset: 0,
                len: len as usize,
                limit: 65_536,
            })
        );

        // Even with raised limits, a 4-byte buffer must return UnexpectedEof or
        // LengthOverflow immediately without wrapping, panicking, or allocating.
        let wide = Limits::new()
            .with_max_opaque_len(u32::MAX)
            .with_max_array_len(u32::MAX)
            .with_max_total_bytes(usize::MAX);

        let err = Decoder::with_limits(&wire, wide)
            .decode_opaque(None)
            .unwrap_err();
        assert!(
            matches!(
                err,
                Error::UnexpectedEof { .. } | Error::LengthOverflow { .. }
            ),
            "len={len}: unexpected error {err:?}"
        );

        let err = Decoder::with_limits(&wire, wide)
            .decode_array(None, |d| d.decode_uint())
            .unwrap_err();
        assert!(
            matches!(
                err,
                Error::UnexpectedEof { .. } | Error::LengthOverflow { .. }
            ),
            "array len={len}: unexpected error {err:?}"
        );
    }
}

#[test]
fn rejects_preallocation_bomb_and_void_array_cpu_bomb() {
    // Pre-allocation bomb: 4-byte buffer claiming 65,536 elements of 128-byte
    // structs must hit UnexpectedEof without allocating 65,536 * 128 bytes.
    let bomb_64k = 65_536u32.to_be_bytes();
    let err = Decoder::new(&bomb_64k)
        .decode_array(None, |d| d.decode_fixed_opaque::<128>())
        .unwrap_err();
    assert!(matches!(err, Error::UnexpectedEof { .. }));

    // Raised max_array_len claiming u32::MAX elements of 1 KiB arrays:
    // incremental pre-allocation cap (256) prevents OOM and hits UnexpectedEof.
    let bomb_max = u32::MAX.to_be_bytes();
    let wide = Limits::new().with_max_array_len(u32::MAX);
    let err = Decoder::with_limits(&bomb_max, wide)
        .decode_array(None, |d| d.decode_fixed_opaque::<1024>())
        .unwrap_err();
    assert!(matches!(err, Error::UnexpectedEof { .. }));

    // Zero-byte void array CPU bomb: 4-byte wire buffer claiming u32::MAX `()`.
    assert_eq!(
        xdr::decode::<Vec<()>>(&[0xff, 0xff, 0xff, 0xff]),
        Err(Error::LengthLimit {
            offset: 0,
            len: u32::MAX as usize,
            limit: 65_536,
        })
    );
}

#[test]
fn rejects_nested_depth_bomb_fast_without_stack_overflow() {
    #[derive(Debug, PartialEq, Eq)]
    struct DeepNode(Option<Box<DeepNode>>);

    impl XdrDecode for DeepNode {
        fn decode(dec: &mut Decoder<'_>) -> Result<Self> {
            Ok(Self(dec.decode_option(|d| d.decode::<Box<DeepNode>>())?))
        }
    }

    // 10,000 nested `Some` tags followed by `None`.
    let mut bomb = Vec::with_capacity(10_001 * 4);
    for _ in 0..10_000 {
        bomb.extend_from_slice(&1u32.to_be_bytes());
    }
    bomb.extend_from_slice(&0u32.to_be_bytes());

    let err = xdr::decode::<DeepNode>(&bomb).unwrap_err();
    assert!(
        matches!(err, Error::DepthLimit { limit: 64, .. }),
        "expected DepthLimit {{ limit: 64, .. }}, got {err:?}"
    );
}

// --- Encoder prefix preservation & rollback to start_len -------------------

#[test]
fn encoder_preserves_existing_prefix_and_counts_limits_from_start_len() {
    let prefix = [0xde, 0xad, 0xbe, 0xef];
    let mut buf = prefix.to_vec();
    let limits = Limits::new().with_max_total_bytes(8);

    let mut enc = Encoder::with_limits(&mut buf, limits);
    assert_eq!(enc.written(), 0);
    assert_eq!(enc.limits(), limits);
    enc.encode_uint(1).unwrap();
    enc.encode_uint(2).unwrap();
    assert_eq!(enc.written(), 8);

    assert_eq!(
        buf,
        [0xde, 0xad, 0xbe, 0xef, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02]
    );
}

#[test]
fn encode_into_truncates_back_to_start_len_on_error_preserving_prefix() {
    struct PartialFailure;

    impl XdrEncode for PartialFailure {
        fn encode(&self, enc: &mut Encoder<'_>) -> Result<()> {
            enc.encode_uint(0x1122_3344)?;
            enc.encode_str("written-before-failure")?;
            // Fails after partial bytes were already appended to the buffer.
            enc.encode_opaque_bounded(&[0xaa; 10], 4)
        }
    }

    let prefix = [0xca, 0xfe, 0xba, 0xbe];
    let mut buf = prefix.to_vec();

    let err = xdr::encode_into(&PartialFailure, &mut buf).unwrap_err();
    assert!(matches!(
        err,
        Error::LengthLimit {
            len: 10,
            limit: 4,
            ..
        }
    ));
    assert_eq!(
        buf.as_slice(),
        &prefix,
        "encode_into must truncate back to start_len and preserve existing prefix"
    );
}

#[test]
fn encoder_nest_and_compounds_truncate_back_to_start_len_on_error() {
    let prefix = [0x11, 0x22, 0x33, 0x44];

    // Failure inside `nest` due to TotalBytesLimit rolls back to start_len.
    let mut buf = prefix.to_vec();
    let limits = Limits::new().with_max_total_bytes(8);
    let mut enc = Encoder::with_limits(&mut buf, limits);
    let err = enc
        .nest(|e| {
            e.encode_uint(1)?;
            e.encode_uint(2)?;
            e.encode_uint(3)
        })
        .unwrap_err();
    assert_eq!(
        err,
        Error::TotalBytesLimit {
            offset: 8,
            limit: 8,
        }
    );
    assert_eq!(enc.written(), 0);
    assert_eq!(buf.as_slice(), &prefix);

    // Failure mid-way through `encode_array` rolls back to start_len.
    let mut buf = prefix.to_vec();
    let limits = Limits::new().with_max_total_bytes(12);
    let mut enc = Encoder::with_limits(&mut buf, limits);
    let err = enc
        .encode_array(&[1u32, 2, 3, 4], None, |e, &v| e.encode_uint(v))
        .unwrap_err();
    assert_eq!(
        err,
        Error::TotalBytesLimit {
            offset: 12,
            limit: 12,
        }
    );
    assert_eq!(enc.written(), 0);
    assert_eq!(buf.as_slice(), &prefix);

    // Failure mid-way through `encode_list` rolls back to start_len.
    let mut buf = prefix.to_vec();
    let limits = Limits::new().with_max_total_bytes(12);
    let mut enc = Encoder::with_limits(&mut buf, limits);
    let err = enc
        .encode_list(&[1u32, 2, 3], None, |e, &v| e.encode_uint(v))
        .unwrap_err();
    assert!(matches!(err, Error::TotalBytesLimit { limit: 12, .. }));
    assert_eq!(enc.written(), 0);
    assert_eq!(buf.as_slice(), &prefix);
}

// --- Property-style round-trip loops (no external crates) ------------------

#[test]
fn property_round_trips_primitives_and_compounds_canonically() {
    let mut rng = Rng::new(0x9e37_79b9_7f4a_7c15);

    for _ in 0..200 {
        let i32_v = rng.next_u32() as i32;
        let u32_v = rng.next_u32();
        let i64_v = rng.next_u64() as i64;
        let u64_v = rng.next_u64();
        let bool_v = (rng.next_u32() & 1) == 1;
        let f32_v = f32::from_bits(rng.next_u32());
        let f64_v = f64::from_bits(rng.next_u64());

        let opaque_len = (rng.next_u32() % 68) as usize;
        let opaque_v = Opaque(rng.next_bytes(opaque_len));

        let fixed5_arr: [u8; 5] = rng.next_bytes(5).try_into().unwrap();
        let fixed5_v = FixedOpaque(fixed5_arr);

        let str_len = (rng.next_u32() % 24) as usize;
        let str_v: String = (0..str_len)
            .map(|_| match rng.next_u32() % 5 {
                0 => 'a',
                1 => '\0',
                2 => '\n',
                3 => 'é',
                _ => '🦀',
            })
            .collect();

        let arr_len = (rng.next_u32() % 12) as usize;
        let arr_v: Vec<i32> = (0..arr_len).map(|_| rng.next_u32() as i32).collect();
        let opt_v = if bool_v { Some(str_v.clone()) } else { None };
        let list_v: Vec<u32> = (0..(rng.next_u32() % 10)).map(|_| rng.next_u32()).collect();

        let mut wire = Vec::new();
        {
            let mut enc = Encoder::new(&mut wire);
            enc.encode(&i32_v).unwrap();
            enc.encode(&u32_v).unwrap();
            enc.encode(&i64_v).unwrap();
            enc.encode(&u64_v).unwrap();
            enc.encode(&bool_v).unwrap();
            enc.encode(&f32_v).unwrap();
            enc.encode(&f64_v).unwrap();
            enc.encode(&opaque_v).unwrap();
            enc.encode(&fixed5_v).unwrap();
            enc.encode(&str_v).unwrap();
            enc.encode(&arr_v).unwrap();
            enc.encode(&opt_v).unwrap();
            enc.encode_list(&list_v, None, |e, &v| e.encode_uint(v))
                .unwrap();
        }
        assert_eq!(
            wire.len() % 4,
            0,
            "XDR output must always be 4-byte aligned"
        );

        let mut dec = Decoder::new(&wire);
        assert_eq!(dec.decode::<i32>().unwrap(), i32_v);
        assert_eq!(dec.decode::<u32>().unwrap(), u32_v);
        assert_eq!(dec.decode::<i64>().unwrap(), i64_v);
        assert_eq!(dec.decode::<u64>().unwrap(), u64_v);
        assert_eq!(dec.decode::<bool>().unwrap(), bool_v);
        assert_eq!(dec.decode::<f32>().unwrap().to_bits(), f32_v.to_bits());
        assert_eq!(dec.decode::<f64>().unwrap().to_bits(), f64_v.to_bits());
        assert_eq!(dec.decode::<Opaque>().unwrap(), opaque_v);
        assert_eq!(dec.decode::<FixedOpaque<5>>().unwrap(), fixed5_v);
        assert_eq!(dec.decode::<String>().unwrap(), str_v);
        assert_eq!(dec.decode::<Vec<i32>>().unwrap(), arr_v);
        assert_eq!(dec.decode::<Option<String>>().unwrap(), opt_v);
        assert_eq!(dec.decode_list(None, |d| d.decode_uint()).unwrap(), list_v);
        dec.finish().unwrap();

        // Re-encoding the decoded values produces byte-for-byte identical output.
        let mut reencoded = Vec::new();
        {
            let mut enc = Encoder::new(&mut reencoded);
            enc.encode(&i32_v).unwrap();
            enc.encode(&u32_v).unwrap();
            enc.encode(&i64_v).unwrap();
            enc.encode(&u64_v).unwrap();
            enc.encode(&bool_v).unwrap();
            enc.encode(&f32_v).unwrap();
            enc.encode(&f64_v).unwrap();
            enc.encode(&opaque_v).unwrap();
            enc.encode(&fixed5_v).unwrap();
            enc.encode(&str_v).unwrap();
            enc.encode(&arr_v).unwrap();
            enc.encode(&opt_v).unwrap();
            enc.encode_list(&list_v, None, |e, &v| e.encode_uint(v))
                .unwrap();
        }
        assert_eq!(reencoded, wire);
    }
}

// --- Error Display and std::error::Error -----------------------------------

#[test]
fn error_display_and_std_error_trait_cover_all_variants() {
    let errors = [
        Error::UnexpectedEof {
            offset: 4,
            needed: 8,
            remaining: 2,
        },
        Error::TrailingBytes {
            offset: 12,
            remaining: 3,
        },
        Error::NonZeroPadding {
            offset: 7,
            byte: 0xab,
        },
        Error::InvalidBool {
            offset: 8,
            value: 2,
        },
        Error::InvalidEnum {
            offset: 16,
            value: -9,
        },
        Error::InvalidDiscriminant {
            offset: 20,
            value: 99,
        },
        Error::InvalidUtf8 {
            offset: 24,
            valid_up_to: 3,
        },
        Error::LengthLimit {
            offset: 28,
            len: 500,
            limit: 64,
        },
        Error::TotalBytesLimit {
            offset: 32,
            limit: 32,
        },
        Error::DepthLimit {
            offset: 36,
            limit: 64,
        },
        Error::LengthOverflow {
            offset: 40,
            len: usize::MAX,
        },
    ];

    for err in errors {
        let msg = err.to_string();
        assert!(!msg.is_empty(), "empty Display for {err:?}");
        let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(err);
        assert_eq!(boxed.to_string(), msg);
    }
}
