//! Integration tests for [`akku::bytes`] — slicing, sharing, endianness and
//! bounds checking.

use akku::bytes::varint::{self, VarintError, MAX_VARINT_LEN64};
use akku::bytes::{Bytes, BytesError, BytesMut, Cursor};
use std::io::Write;

#[test]
fn static_bytes_slice_without_allocation() {
    let base = Bytes::from_static(b"hello akku");
    assert_eq!(base.len(), 10);
    assert!(!base.is_empty());

    let word = base.slice(6..);
    assert_eq!(word.as_slice(), b"akku");
    assert_eq!(base.as_slice(), b"hello akku", "original stays untouched");

    let ell = base.slice(1..4);
    assert_eq!(ell.as_slice(), b"ell");
    assert_eq!(base.slice(..2).as_slice(), b"he");
    assert_eq!(base.slice(5..=9).as_slice(), b" akku");
    assert_eq!(base.slice(..).len(), base.len());
    assert_eq!(base.slice(3..3).len(), 0);
}

#[test]
fn static_bytes_split_to_and_off_partition_the_window() {
    let mut buffer = Bytes::from_static(b"abcdef");
    let head = buffer.split_to(2);
    assert_eq!(head.as_slice(), b"ab");
    assert_eq!(buffer.as_slice(), b"cdef");

    let tail = buffer.split_off(1);
    assert_eq!(buffer.as_slice(), b"c");
    assert_eq!(tail.as_slice(), b"def");

    // Empty window after repeated split_to calls.
    let mut empty = Bytes::from_static(b"x");
    let _ = empty.split_to(1);
    assert!(empty.is_empty());
    assert_eq!(empty.slice(..), Bytes::new());
}

#[test]
fn shared_bytes_clones_are_independent_views() {
    // One owner: dropping `base` alone must not free shared storage.
    let mut base = Bytes::from(vec![1u8, 2, 3, 4, 5, 6]);

    let clone = base.clone();
    let taken = base.split_to(2);

    assert_eq!(taken.as_slice(), &[1, 2]);
    assert_eq!(base.as_slice(), &[3, 4, 5, 6]);
    assert_eq!(
        clone.as_slice(),
        &[1, 2, 3, 4, 5, 6],
        "older clone keeps the full window"
    );

    let inner = clone.slice(2..4);
    assert_eq!(inner.as_slice(), &[3, 4]);

    let mut truncated = clone.clone();
    truncated.truncate(3);
    assert_eq!(truncated.as_slice(), &[1, 2, 3]);
    assert_eq!(clone.as_slice(), &[1, 2, 3, 4, 5, 6]);

    let mut cleared = base.clone();
    cleared.clear();
    assert!(cleared.is_empty());
    assert_eq!(base.as_slice(), &[3, 4, 5, 6]);
}

#[test]
fn bytes_equality_ordering_hash_and_conversions() {
    let a = Bytes::copy_from_slice(b"abc");
    let b = Bytes::from(vec![b'a', b'b', b'c']);
    let c = Bytes::copy_from_slice(b"abd");

    assert_eq!(a, b);
    assert_ne!(a, c);
    assert!(a < c);
    assert_eq!(a.as_slice(), b"abc"); // PartialEq<[u8]>
    assert_eq!(a, &b"abc"[..]); // PartialEq<&[u8]>
    assert_eq!(a, vec![b'a', b'b', b'c']); // PartialEq<Vec<u8>>

    let hash_of = |bytes: &Bytes| {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        hasher.finish()
    };
    assert_eq!(hash_of(&a), hash_of(&b));

    assert_eq!(Bytes::from("static str").as_slice(), b"static str");
    assert_eq!(
        Bytes::from(String::from("owned str")).as_slice(),
        b"owned str"
    );
    assert_eq!(Bytes::new().to_vec(), Vec::<u8>::new());
    assert!(Bytes::default().is_empty());
    assert!(!format!("{a:?}").is_empty());
}

#[test]
#[should_panic(expected = "slice range")]
fn slice_out_of_bounds_panics() {
    let base = Bytes::from_static(b"abc");
    let _ = base.slice(2..5);
}

#[test]
#[should_panic(expected = "split_to")]
fn split_to_out_of_bounds_panics() {
    let mut base = Bytes::from_static(b"abc");
    let _ = base.split_to(4);
}

#[test]
#[should_panic(expected = "split_off")]
fn split_off_out_of_bounds_panics() {
    let mut base = Bytes::from_static(b"abc");
    let _ = base.split_off(4);
}

#[test]
fn bytes_mut_endianness_roundtrips_through_cursor() {
    let mut buf = BytesMut::with_capacity(64);

    buf.put_u8(0x7F);
    buf.put_u16(0x0102);
    buf.put_u16_le(0x0102);
    buf.put_u32(0x01020304);
    buf.put_u32_le(0x01020304);
    buf.put_u64(0x0102030405060708);
    buf.put_u64_le(0x0102030405060708);
    buf.put_u128(0x0102030405060708090A0B0C0D0E0F10);
    buf.put_u128_le(0x0102030405060708090A0B0C0D0E0F10);
    buf.put_i16(-2);
    buf.put_i16_le(-2);
    buf.put_i32(-2);
    buf.put_i32_le(-2);
    buf.put_i64(-2);
    buf.put_i64_le(-2);
    buf.put_i128(-2);
    buf.put_i128_le(-2);
    buf.put_f32(1.5);
    buf.put_f32_le(1.5);
    buf.put_f64(-2.25);
    buf.put_f64_le(-2.25);
    buf.put_slice(b"tail");

    let mut reader = Cursor::new(&buf);
    assert_eq!(reader.get_u8().unwrap(), 0x7F);
    assert_eq!(reader.get_u16().unwrap(), 0x0102);
    assert_eq!(reader.get_u16_le().unwrap(), 0x0102);
    assert_eq!(reader.get_u32().unwrap(), 0x01020304);
    assert_eq!(reader.get_u32_le().unwrap(), 0x01020304);
    assert_eq!(reader.get_u64().unwrap(), 0x0102030405060708);
    assert_eq!(reader.get_u64_le().unwrap(), 0x0102030405060708);
    assert_eq!(
        reader.get_u128().unwrap(),
        0x0102030405060708090A0B0C0D0E0F10
    );
    assert_eq!(
        reader.get_u128_le().unwrap(),
        0x0102030405060708090A0B0C0D0E0F10
    );
    assert_eq!(reader.get_i16().unwrap(), -2);
    assert_eq!(reader.get_i16_le().unwrap(), -2);
    assert_eq!(reader.get_i32().unwrap(), -2);
    assert_eq!(reader.get_i32_le().unwrap(), -2);
    assert_eq!(reader.get_i64().unwrap(), -2);
    assert_eq!(reader.get_i64_le().unwrap(), -2);
    assert_eq!(reader.get_i128().unwrap(), -2);
    assert_eq!(reader.get_i128_le().unwrap(), -2);
    assert_eq!(reader.get_f32().unwrap(), 1.5);
    assert_eq!(reader.get_f32_le().unwrap(), 1.5);
    assert_eq!(reader.get_f64().unwrap(), -2.25);
    assert_eq!(reader.get_f64_le().unwrap(), -2.25);
    assert_eq!(reader.get_slice(4).unwrap(), b"tail");

    assert!(reader.is_empty());
    assert_eq!(reader.remaining(), 0);
    assert_eq!(reader.position(), buf.len());
}

#[test]
fn cursor_reports_underflow_as_error_values() {
    let mut reader = Cursor::new(&[0x01, 0x02, 0x03]);
    assert_eq!(reader.remaining(), 3);

    reader.advance(2).expect("2 bytes are available");
    assert_eq!(reader.position(), 2);

    let error = reader.get_u16().unwrap_err();
    assert_eq!(
        error,
        BytesError {
            needed: 2,
            remaining: 1
        }
    );
    assert_eq!(
        error.to_string(),
        "unexpected end of buffer: wanted 2 byte(s), 1 remaining"
    );

    let error = reader.get_slice(3).unwrap_err();
    assert_eq!(
        error,
        BytesError {
            needed: 3,
            remaining: 1
        }
    );

    let error = reader.advance(2).unwrap_err();
    assert_eq!(
        error,
        BytesError {
            needed: 2,
            remaining: 1
        }
    );

    // A failed read leaves the position untouched.
    assert_eq!(reader.position(), 2);

    let mut fresh = Cursor::new(&[]);
    let error = fresh.get_u8().unwrap_err();
    assert_eq!(
        error,
        BytesError {
            needed: 1,
            remaining: 0
        }
    );
    let error = fresh.peek_u8().unwrap_err();
    assert_eq!(
        error,
        BytesError {
            needed: 1,
            remaining: 0
        }
    );
    assert!(
        fresh.advance(0).is_ok(),
        "advancing zero bytes is always fine"
    );
}

#[test]
fn peek_does_not_advance() {
    let mut reader = Cursor::new(&[0xAB]);
    assert_eq!(reader.peek_u8().unwrap(), 0xAB);
    assert_eq!(reader.remaining(), 1);
    assert_eq!(reader.get_u8().unwrap(), 0xAB);
    assert!(reader.is_empty());
}

#[test]
fn bytes_mut_io_write_accumulates_bytes() -> std::io::Result<()> {
    let mut buf = BytesMut::new();
    assert_eq!(buf.write(b"hello ")?, 6);
    write!(buf, "{}", 42)?;
    buf.write_all(b"!").expect("write_all grows the buffer");
    assert_eq!(buf.write(b"")?, 0);

    assert_eq!(buf.as_ref(), b"hello 42!");
    assert_eq!(buf.len(), 9);
    assert!(buf.capacity() >= buf.len());
    buf.flush().expect("flush is a no-op");

    buf.clear();
    assert!(buf.is_empty());
    Ok(())
}

#[test]
fn bytes_mut_splits_and_freezes() {
    let mut buf = BytesMut::new();
    buf.extend_from_slice(b"abcdef");
    let head = buf.split_to(2);
    assert_eq!(head.as_ref(), b"ab");
    assert_eq!(buf.as_ref(), b"cdef");

    let tail = buf.split_off(1);
    assert_eq!(buf.as_ref(), b"c");
    assert_eq!(tail.as_ref(), b"def");

    buf.extend_from_slice(b"def");
    let frozen = buf.freeze();
    assert_eq!(frozen.as_slice(), b"cdef");
    assert_eq!(frozen.slice(1..3).as_slice(), b"de");
}

#[test]
fn freeze_shares_one_allocation_with_every_clone_and_slice() {
    let mut buf = BytesMut::new();
    buf.extend_from_slice(b"hello akku");
    let frozen = buf.freeze();

    let base = frozen.as_ptr();
    let second = frozen.clone();
    let part = frozen.slice(6..);

    assert_eq!(second.as_ptr(), base, "clone reuses the frozen allocation");
    assert_eq!(
        part.as_ptr().wrapping_offset(-6),
        base,
        "slicing offsets into the same allocation instead of copying"
    );

    assert_eq!(part.as_slice(), b"akku");
    assert_eq!(second.as_slice(), b"hello akku");
    assert_eq!(frozen.as_slice(), b"hello akku");

    let empty = BytesMut::new().freeze();
    assert!(empty.is_empty());
    assert_eq!(empty.as_slice(), b"");
}

#[test]
fn emptied_bytes_do_not_pin_their_storage() {
    let mut big = Bytes::from(vec![7u8; 4 * 1024 * 1024]);
    big.clear();
    assert!(big.is_empty());
    assert_eq!(big.len(), 0);
    assert_eq!(big.as_slice(), b"");
    assert_eq!(
        big,
        Bytes::new(),
        "an emptied window equals the static empty"
    );
    assert_eq!(big.slice(..).len(), 0);

    let mut drained = Bytes::copy_from_slice(b"abc");
    let all = drained.split_to(3);
    assert_eq!(all.as_slice(), b"abc");
    assert_eq!(drained, Bytes::new());

    let mut truncated = Bytes::copy_from_slice(b"abcdef");
    truncated.truncate(0);
    assert_eq!(truncated, Bytes::new());

    let mut peeled = Bytes::copy_from_slice(b"xyz");
    let tail = peeled.split_off(0);
    assert_eq!(peeled, Bytes::new());
    assert_eq!(tail.as_slice(), b"xyz");

    // Other clones keep the storage alive for themselves.
    let mut shared = Bytes::copy_from_slice(b"keep me");
    let keeper = shared.clone();
    shared.clear();
    assert!(shared.is_empty());
    assert_eq!(keeper.as_slice(), b"keep me");
}

#[test]
#[should_panic(expected = "split_to")]
fn bytes_mut_split_to_out_of_bounds_panics() {
    let mut buf = BytesMut::new();
    let _ = buf.split_to(1);
}

#[test]
#[should_panic(expected = "split_off")]
fn bytes_mut_split_off_out_of_bounds_panics() {
    let mut buf = BytesMut::new();
    let _ = buf.split_off(1);
}

// ---------------------------------------------------------------------------
// Varints (LEB128 / zigzag). The `go_*` tests reproduce vectors from Go's
// `encoding/binary/varint_test.go` (Copyright 2011 The Go Authors, BSD-3-Clause);
// the other vectors are original or from the protobuf encoding guide.
// ---------------------------------------------------------------------------

/// Deterministic xorshift64* stream for sweeps without a dependency.
fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    state.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

#[test]
fn varint_max_len_constant_matches_go() {
    assert_eq!(MAX_VARINT_LEN64, 10);
    assert_eq!(varint::encoded_len_u64(u64::MAX), MAX_VARINT_LEN64);
}

#[test]
fn uvarint_known_encodings() {
    let cases: &[(u64, &[u8])] = &[
        (0, &[0x00]),
        (1, &[0x01]),
        (127, &[0x7f]),
        (128, &[0x80, 0x01]),
        (150, &[0x96, 0x01]),
        (300, &[0xac, 0x02]),
        (16_383, &[0xff, 0x7f]),
        (16_384, &[0x80, 0x80, 0x01]),
        (
            i64::MAX as u64,
            &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f],
        ),
        (
            1 << 63,
            &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01],
        ),
        (
            u64::MAX,
            &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01],
        ),
    ];
    for &(value, bytes) in cases {
        let mut out = Vec::new();
        varint::append_u64(&mut out, value);
        assert_eq!(out, bytes, "append {value}");
        assert_eq!(varint::encoded_len_u64(value), bytes.len());
        assert_eq!(
            varint::decode_u64(bytes),
            Ok((value, bytes.len())),
            "{value}"
        );
        assert_eq!(varint::decode_u64_strict(bytes), Ok((value, bytes.len())));

        let mut buf = [0xeeu8; MAX_VARINT_LEN64];
        assert_eq!(varint::encode_u64(&mut buf, value), Ok(bytes.len()));
        assert_eq!(&buf[..bytes.len()], bytes);
        assert!(buf[bytes.len()..].iter().all(|&b| b == 0xee), "no overrun");
    }
}

#[test]
fn varint_signed_known_encodings_zigzag() {
    // protobuf guide: 0->0, -1->1, 1->2, -2->3, 2147483647->4294967294.
    assert_eq!(varint::zigzag_encode(0), 0);
    assert_eq!(varint::zigzag_encode(-1), 1);
    assert_eq!(varint::zigzag_encode(1), 2);
    assert_eq!(varint::zigzag_encode(-2), 3);
    assert_eq!(varint::zigzag_encode(2_147_483_647), 4_294_967_294);
    assert_eq!(varint::zigzag_encode(i64::MAX), u64::MAX - 1);
    assert_eq!(varint::zigzag_encode(i64::MIN), u64::MAX);
    assert_eq!(varint::zigzag_decode(u64::MAX), i64::MIN);
    assert_eq!(varint::zigzag_decode(u64::MAX - 1), i64::MAX);

    let cases: &[(i64, &[u8])] = &[
        (0, &[0x00]),
        (-1, &[0x01]),
        (1, &[0x02]),
        (-64, &[0x7f]),
        (64, &[0x80, 0x01]),
        (-65, &[0x81, 0x01]),
    ];
    for &(value, bytes) in cases {
        let mut out = Vec::new();
        varint::append_i64(&mut out, value);
        assert_eq!(out, bytes, "append {value}");
        assert_eq!(varint::decode_i64(bytes), Ok((value, bytes.len())));
        let mut buf = [0u8; MAX_VARINT_LEN64];
        assert_eq!(varint::encode_i64(&mut buf, value), Ok(bytes.len()));
        assert_eq!(&buf[..bytes.len()], bytes);
    }
}

#[test]
fn varint_go_signed_vector_list_round_trips() {
    // The value list from Go's TestVarint tests, plus negations.
    let mut values: Vec<i64> = vec![
        i64::MIN,
        i64::MIN + 1,
        -1,
        0,
        1,
        2,
        10,
        20,
        63,
        64,
        65,
        127,
        128,
        129,
        255,
        256,
        257,
        i64::MAX,
    ];
    let negated: Vec<i64> = values
        .iter()
        .filter(|&&v| v != i64::MIN)
        .map(|&v| -v)
        .collect();
    values.extend(negated);
    for value in values {
        let mut out = Vec::new();
        varint::append_i64(&mut out, value);
        assert!(out.len() <= MAX_VARINT_LEN64);
        assert_eq!(varint::decode_i64(&out), Ok((value, out.len())));
        assert_eq!(varint::decode_i64_strict(&out), Ok((value, out.len())));
    }
}

#[test]
fn varint_decode_reports_consumed_with_trailing_data() {
    let data = [0xac, 0x02, 0xde, 0xad];
    assert_eq!(varint::decode_u64(&data), Ok((300, 2)));
    assert_eq!(varint::decode_i64(&data), Ok((150, 2)));
}

#[test]
fn varint_empty_and_truncated_inputs() {
    assert_eq!(varint::decode_u64(&[]), Err(VarintError::Truncated));
    assert_eq!(varint::decode_i64(&[]), Err(VarintError::Truncated));
    assert_eq!(varint::decode_u64(&[0x80]), Err(VarintError::Truncated));
    assert_eq!(
        varint::decode_u64(&[0xff, 0xff]),
        Err(VarintError::Truncated)
    );
    // Exactly ten continuation bytes: still "more may follow", as in Go (n == 0).
    assert_eq!(varint::decode_u64(&[0x80; 10]), Err(VarintError::Truncated));
    assert_eq!(varint::decode_u64_strict(&[]), Err(VarintError::Truncated));
}

#[test]
fn varint_overflow_inputs() {
    // Overflow shapes in the spirit of Go's TestOverflow.
    let mut eleven = [0x80u8; 11];
    eleven[10] = 0x01;
    assert_eq!(varint::decode_u64(&eleven), Err(VarintError::Overflow));
    assert_eq!(varint::decode_u64(&[0x80; 11]), Err(VarintError::Overflow));
    let mut tenth = [0xffu8; 10];
    tenth[9] = 0x02;
    assert_eq!(varint::decode_u64(&tenth), Err(VarintError::Overflow));
    tenth[9] = 0x7f;
    assert_eq!(varint::decode_u64(&tenth), Err(VarintError::Overflow));
    assert_eq!(varint::decode_i64(&tenth), Err(VarintError::Overflow));
    assert_eq!(
        varint::decode_u64_strict(&tenth),
        Err(VarintError::Overflow)
    );
    // 10th byte == 1 is the largest legal value.
    tenth[9] = 0x01;
    assert_eq!(varint::decode_u64(&tenth), Ok((u64::MAX, 10)));
}

#[test]
fn varint_non_canonical_lenient_accepts_strict_rejects() {
    // Overlong zero (like Go's TestNonCanonicalZero) and protobuf-style padding.
    let cases: &[(&[u8], u64)] = &[
        (&[0x80, 0x00], 0),
        (&[0x80, 0x80, 0x80, 0x00], 0),
        (&[0x81, 0x00], 1),
        (&[0xff, 0x80, 0x00], 127),
        (
            &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x00],
            0,
        ),
    ];
    for &(bytes, value) in cases {
        assert_eq!(
            varint::decode_u64(bytes),
            Ok((value, bytes.len())),
            "{bytes:?}"
        );
        assert_eq!(
            varint::decode_u64_strict(bytes),
            Err(VarintError::NonCanonical)
        );
    }
    assert_eq!(
        varint::decode_i64_strict(&[0x80, 0x00]),
        Err(VarintError::NonCanonical)
    );
    assert_eq!(varint::decode_i64(&[0x81, 0x00]), Ok((-1, 2)));
    // A lone zero byte is canonical.
    assert_eq!(varint::decode_u64_strict(&[0x00]), Ok((0, 1)));
}

#[test]
fn varint_encode_into_too_small_buffer_fails_without_writing() {
    let mut buf = [0xaau8; 1];
    assert_eq!(
        varint::encode_u64(&mut buf, 300),
        Err(VarintError::BufferTooSmall {
            needed: 2,
            available: 1
        })
    );
    assert_eq!(buf, [0xaa], "no partial write");
    assert_eq!(
        varint::encode_u64(&mut [], 0),
        Err(VarintError::BufferTooSmall {
            needed: 1,
            available: 0
        })
    );
    let mut nine = [0u8; 9];
    assert_eq!(
        varint::encode_i64(&mut nine, i64::MIN),
        Err(VarintError::BufferTooSmall {
            needed: 10,
            available: 9
        })
    );
    let mut exact = [0u8; 2];
    assert_eq!(varint::encode_u64(&mut exact, 300), Ok(2));
}

#[test]
fn varint_pseudo_random_sweep_round_trips() {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut stream = Vec::new();
    let mut expected = Vec::new();
    for round in 0..20_000u32 {
        // Vary magnitude so every encoded length 1..=10 is hit.
        let value = xorshift(&mut state) >> (round % 64);
        expected.push(value);
        let mut one = Vec::new();
        varint::append_u64(&mut one, value);
        assert_eq!(one.len(), varint::encoded_len_u64(value));
        assert_eq!(varint::decode_u64(&one), Ok((value, one.len())));
        assert_eq!(varint::decode_u64_strict(&one), Ok((value, one.len())));
        let signed = value as i64;
        let mut s = Vec::new();
        varint::append_i64(&mut s, signed);
        assert_eq!(varint::decode_i64(&s), Ok((signed, s.len())));
        assert_eq!(varint::zigzag_decode(varint::zigzag_encode(signed)), signed);
        stream.extend_from_slice(&one);
    }
    // The concatenated stream decodes back value by value.
    let mut rest = stream.as_slice();
    for want in expected {
        let (got, n) = varint::decode_u64(rest).unwrap();
        assert_eq!(got, want);
        rest = &rest[n..];
    }
    assert!(rest.is_empty());
}

#[test]
fn varint_decoder_never_panics_on_arbitrary_bytes() {
    let mut state = 12345u64;
    for _ in 0..20_000 {
        let len = (xorshift(&mut state) % 14) as usize;
        let data: Vec<u8> = (0..len).map(|_| xorshift(&mut state) as u8).collect();
        let _ = varint::decode_u64(&data);
        let _ = varint::decode_i64(&data);
        let _ = varint::decode_u64_strict(&data);
        let _ = varint::decode_i64_strict(&data);
    }
}

#[test]
fn varint_bytesmut_put_and_cursor_get() {
    let mut buf = BytesMut::new();
    buf.put_uvarint(300);
    buf.put_varint(-1);
    buf.put_uvarint(u64::MAX);
    buf.put_varint(i64::MIN);
    assert_eq!(&buf[..4], &[0xac, 0x02, 0x01, 0xff]);

    let mut reader = Cursor::new(&buf);
    assert_eq!(reader.get_uvarint(), Ok(300));
    assert_eq!(reader.get_varint(), Ok(-1));
    assert_eq!(reader.get_uvarint(), Ok(u64::MAX));
    assert_eq!(reader.get_varint(), Ok(i64::MIN));
    assert!(reader.is_empty());
    assert_eq!(reader.get_uvarint(), Err(VarintError::Truncated));
}

#[test]
fn varint_cursor_error_leaves_position_unchanged() {
    let data = [0x07, 0x80];
    let mut reader = Cursor::new(&data);
    assert_eq!(reader.get_uvarint(), Ok(7));
    assert_eq!(reader.get_uvarint(), Err(VarintError::Truncated));
    assert_eq!(reader.position(), 1);

    let bad = [0xffu8; 11];
    let mut reader = Cursor::new(&bad);
    assert_eq!(reader.get_uvarint(), Err(VarintError::Overflow));
    assert_eq!(reader.position(), 0);
}

#[test]
fn varint_error_display_and_std_error() {
    let err: Box<dyn std::error::Error> = Box::new(VarintError::Overflow);
    assert!(err.to_string().contains("overflow"));
    assert!(VarintError::Truncated.to_string().contains("truncated"));
    assert!(VarintError::NonCanonical.to_string().contains("canonical"));
    let small = VarintError::BufferTooSmall {
        needed: 2,
        available: 1,
    };
    assert!(small.to_string().contains('2'));
}

#[test]
fn varint_go_overflow_vectors_verbatim() {
    // Verbatim from Go's TestOverflow and TestBufferTooBigWithOverflow.
    assert_eq!(
        varint::decode_u64(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02]),
        Err(VarintError::Overflow)
    );
    assert_eq!(
        varint::decode_u64(&[
            0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01, 0, 0
        ]),
        Err(VarintError::Overflow)
    );
    assert_eq!(varint::decode_u64(&[0xff; 11]), Err(VarintError::Overflow));
    let mut long = vec![0xffu8; 1000];
    long[999] = 0;
    assert_eq!(varint::decode_u64(&long), Err(VarintError::Overflow));
    assert_eq!(
        varint::decode_u64(&[0xd7, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]),
        Ok((u64::MAX - 40, 10))
    );
    assert_eq!(
        varint::decode_u64(&[0xd7, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]),
        Err(VarintError::Overflow)
    );
    assert_eq!(
        varint::decode_u64(&[0xd7, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f]),
        Err(VarintError::Overflow)
    );
}

#[test]
fn varint_go_buffer_too_small_vectors_verbatim() {
    // Go's TestBufferTooSmall: prefixes of 0x80 x4 are all truncated.
    let buf = [0x80u8; 4];
    for i in 0..=buf.len() {
        assert_eq!(varint::decode_u64(&buf[..i]), Err(VarintError::Truncated));
    }
}
