//! Integration tests for [`akku::bytes`] — slicing, sharing, endianness and
//! bounds checking.

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
