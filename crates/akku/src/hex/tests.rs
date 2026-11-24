//! Public-API tests for `akku::hex`. Seams are listed in `_dev/DESIGN.md`.

use akku::hex::{self, BufferTooSmall, DecodeError};

/// The 22 bytes accepted as hex digits, written out by hand.
const HEX_DIGITS: &[u8] = b"0123456789abcdefABCDEF";

fn invalid(index: usize, byte: u8) -> Result<Vec<u8>, DecodeError> {
    Err(DecodeError::InvalidByte { index, byte })
}

/// Deterministic xorshift64 bytes, so large-input tests need no RNG dependency.
fn pseudo_random_bytes(len: usize) -> Vec<u8> {
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 56) as u8
        })
        .collect()
}

// --- Known-answer vectors --------------------------------------------------

#[test]
fn matches_rfc4648_section_10_base16_vectors() {
    // RFC 4648 §10 lists these with uppercase output.
    let vectors: &[(&[u8], &str, &str)] = &[
        (b"", "", ""),
        (b"f", "66", "66"),
        (b"fo", "666f", "666F"),
        (b"foo", "666f6f", "666F6F"),
        (b"foob", "666f6f62", "666F6F62"),
        (b"fooba", "666f6f6261", "666F6F6261"),
        (b"foobar", "666f6f626172", "666F6F626172"),
    ];

    for &(plain, lower, upper) in vectors {
        assert_eq!(hex::encoded_len(plain.len()), lower.len());
        assert_eq!(hex::encode(plain), lower);
        assert_eq!(hex::encode_upper(plain), upper);
        assert_eq!(hex::decode(lower).unwrap(), plain);
        assert_eq!(hex::decode(upper).unwrap(), plain);
    }
}

#[test]
fn encodes_nibble_boundary_bytes() {
    let input = [0x00, 0x09, 0x0a, 0x0f, 0x10, 0x90, 0xa0, 0xf0, 0xff];
    assert_eq!(hex::encode(input), "00090a0f1090a0f0ff");
    assert_eq!(hex::encode_upper(input), "00090A0F1090A0F0FF");
}

#[test]
fn encodes_high_nibble_first() {
    assert_eq!(hex::encode([0x12]), "12");
    assert_eq!(hex::decode("12").unwrap(), [0x12]);
    assert_eq!(hex::decode("21").unwrap(), [0x21]);
}

#[test]
fn accepts_str_string_slice_vec_and_array_inputs() {
    let expected = vec![0xab, 0xcd];
    assert_eq!(hex::decode("abcd").unwrap(), expected);
    assert_eq!(hex::decode(String::from("abcd")).unwrap(), expected);
    assert_eq!(hex::decode(&b"abcd"[..]).unwrap(), expected);
    let owned: Vec<u8> = b"abcd".to_vec();
    assert_eq!(hex::decode(owned).unwrap(), expected);
    assert_eq!(hex::decode(*b"abcd").unwrap(), expected);
    assert_eq!(hex::encode("\u{ab}"), "c2ab");
    assert_eq!(hex::encode(vec![0xab_u8]), "ab");
}

// --- Empty input -----------------------------------------------------------

#[test]
fn empty_input_encodes_and_decodes_to_empty() {
    assert_eq!(hex::encoded_len(0), 0);
    assert_eq!(hex::encode(b""), "");
    assert_eq!(hex::encode_upper(b""), "");
    assert_eq!(hex::decode(""), Ok(Vec::new()));
    assert_eq!(hex::encode_to_slice(b"", &mut []), Ok(0));
    assert_eq!(hex::encode_upper_to_slice(b"", &mut []), Ok(0));
    assert_eq!(hex::decode_to_slice("", &mut []), Ok(0));
}

// --- Letter case -----------------------------------------------------------

#[test]
fn decodes_lowercase_uppercase_and_mixed_case_per_nibble() {
    let expected = [0xab, 0xab, 0xab, 0xab];
    assert_eq!(hex::decode("abABaBAb").unwrap(), expected);
    assert_eq!(hex::decode("DeAdBeEf").unwrap(), [0xde, 0xad, 0xbe, 0xef]);
    assert_eq!(hex::decode("fF").unwrap(), [0xff]);
}

#[test]
fn uppercase_encoders_emit_only_uppercase_letters() {
    let all: Vec<u8> = (0u8..=255).collect();
    let upper = hex::encode_upper(&all);
    assert!(upper.bytes().all(|b| b"0123456789ABCDEF".contains(&b)));
    let lower = hex::encode(&all);
    assert!(lower.bytes().all(|b| b"0123456789abcdef".contains(&b)));
}

// --- Every byte value ------------------------------------------------------

#[test]
fn accepts_exactly_the_22_hex_digit_bytes() {
    for byte in 0u8..=255 {
        let result = hex::decode([b'0', byte]);
        if HEX_DIGITS.contains(&byte) {
            // Reference value from std, independent of the decoder.
            let expected = u8::from_str_radix(&(byte as char).to_string(), 16).unwrap();
            assert_eq!(result, Ok(vec![expected]), "byte 0x{byte:02x}");
        } else {
            assert_eq!(result, invalid(1, byte), "byte 0x{byte:02x}");
        }
    }
}

#[test]
fn all_256_byte_values_round_trip_in_both_cases() {
    let all: Vec<u8> = (0u8..=255).collect();
    let lower = hex::encode(&all);
    let upper = hex::encode_upper(&all);

    assert_eq!(lower.len(), 512);
    assert!(lower.starts_with("000102030405060708090a0b0c0d0e0f10"));
    assert!(lower.ends_with("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff"));
    assert!(upper.ends_with("F0F1F2F3F4F5F6F7F8F9FAFBFCFDFEFF"));
    assert_eq!(hex::decode(&lower).unwrap(), all);
    assert_eq!(hex::decode(&upper).unwrap(), all);
}

// --- Invalid bytes at every position class -----------------------------------

#[test]
fn rejects_bytes_just_outside_each_digit_range() {
    // Neighbours of '0'..='9', 'A'..='F', 'a'..='f' in ASCII.
    for &byte in b"/:@G`g" {
        assert_eq!(hex::decode([byte, b'0']), invalid(0, byte));
        assert_eq!(hex::decode([b'0', byte]), invalid(1, byte));
    }
}

#[test]
fn rejects_invalid_byte_in_high_and_low_nibble_of_any_pair() {
    // first pair, high nibble
    assert_eq!(hex::decode("x0112233"), invalid(0, b'x'));
    // first pair, low nibble
    assert_eq!(hex::decode("0x112233"), invalid(1, b'x'));
    // middle pair, high nibble
    assert_eq!(hex::decode("0011x233"), invalid(4, b'x'));
    // middle pair, low nibble
    assert_eq!(hex::decode("00112x33"), invalid(5, b'x'));
    // last pair, high nibble
    assert_eq!(hex::decode("001122x3"), invalid(6, b'x'));
    // last pair, low nibble (very last byte)
    assert_eq!(hex::decode("0011223x"), invalid(7, b'x'));
}

#[test]
fn reports_the_first_of_several_invalid_bytes() {
    assert_eq!(hex::decode("0g0h"), invalid(1, b'g'));
    assert_eq!(hex::decode("zz"), invalid(0, b'z'));
}

#[test]
fn rejects_whitespace_separators_prefixes_and_signs() {
    assert_eq!(hex::decode(" 00"), invalid(0, b' '));
    assert_eq!(hex::decode("00 "), invalid(2, b' '));
    assert_eq!(hex::decode("00\n"), invalid(2, b'\n'));
    assert_eq!(hex::decode("00\t11"), invalid(2, b'\t'));
    assert_eq!(hex::decode("de:ad"), invalid(2, b':'));
    assert_eq!(hex::decode("de-ad"), invalid(2, b'-'));
    assert_eq!(hex::decode("0x00"), invalid(1, b'x'));
    assert_eq!(hex::decode("+1"), invalid(0, b'+'));
    assert_eq!(hex::decode("-1"), invalid(0, b'-'));
}

#[test]
fn rejects_control_nul_and_high_bytes() {
    assert_eq!(hex::decode([0x00, b'0']), invalid(0, 0x00));
    assert_eq!(hex::decode([b'0', 0x7f]), invalid(1, 0x7f));
    assert_eq!(hex::decode([b'0', 0x80]), invalid(1, 0x80));
    assert_eq!(hex::decode([0xff, 0xff]), invalid(0, 0xff));
}

#[test]
fn reports_first_utf8_byte_of_non_ascii_characters() {
    // 'é' is C3 A9 in UTF-8.
    assert_eq!(hex::decode("é"), invalid(0, 0xc3));
    assert_eq!(hex::decode("0é1"), invalid(1, 0xc3));
    // Fullwidth digit zero U+FF10 is EF BC 90.
    assert_eq!(hex::decode("\u{ff10}0"), invalid(0, 0xef));
}

// --- Odd length and error precedence -----------------------------------------

#[test]
fn rejects_odd_length_of_valid_digits() {
    assert_eq!(hex::decode("0"), Err(DecodeError::OddLength { len: 1 }));
    assert_eq!(hex::decode("abc"), Err(DecodeError::OddLength { len: 3 }));
    assert_eq!(hex::decode("ABCDE"), Err(DecodeError::OddLength { len: 5 }));
}

#[test]
fn invalid_byte_wins_over_odd_length_at_any_position() {
    assert_eq!(hex::decode("g"), invalid(0, b'g'));
    assert_eq!(hex::decode("gbc"), invalid(0, b'g'));
    assert_eq!(hex::decode("agc"), invalid(1, b'g'));
    // The dangling last byte of an odd input is also checked (Go does the same).
    assert_eq!(hex::decode("abg"), invalid(2, b'g'));
    assert_eq!(hex::decode("a?cde"), invalid(1, b'?'));
}

#[test]
fn decode_to_slice_error_precedence_is_invalid_then_odd_then_too_small() {
    let mut small = [0u8; 1];
    // all three problems: invalid byte wins
    assert_eq!(
        hex::decode_to_slice("abcdz", &mut small),
        Err(DecodeError::InvalidByte {
            index: 4,
            byte: b'z'
        })
    );
    // odd + too small: odd wins
    assert_eq!(
        hex::decode_to_slice("abcde", &mut small),
        Err(DecodeError::OddLength { len: 5 })
    );
    // invalid + too small: invalid wins, even at the last byte
    assert_eq!(
        hex::decode_to_slice("deaz", &mut small),
        Err(DecodeError::InvalidByte {
            index: 3,
            byte: b'z'
        })
    );
    // odd with a large enough buffer: still odd
    let mut big = [0u8; 8];
    assert_eq!(
        hex::decode_to_slice("abc", &mut big),
        Err(DecodeError::OddLength { len: 3 })
    );
}

// --- `_to_slice` buffer sizes ------------------------------------------------

#[test]
fn encode_to_slice_rejects_one_byte_short_and_leaves_output_untouched() {
    let mut out = [0x99u8; 5];
    assert_eq!(
        hex::encode_to_slice([0xab, 0xcd, 0xef], &mut out),
        Err(BufferTooSmall { needed: 6 })
    );
    assert_eq!(
        hex::encode_upper_to_slice([0xab, 0xcd, 0xef], &mut out),
        Err(BufferTooSmall { needed: 6 })
    );
    assert_eq!(out, [0x99u8; 5]);
}

#[test]
fn encode_to_slice_rejects_empty_output_for_non_empty_input() {
    assert_eq!(
        hex::encode_to_slice([0x00], &mut []),
        Err(BufferTooSmall { needed: 2 })
    );
}

#[test]
fn encode_to_slice_fills_exact_buffer() {
    let mut out = [0u8; 4];
    assert_eq!(hex::encode_to_slice([0xab, 0x01], &mut out), Ok(4));
    assert_eq!(&out, b"ab01");
    assert_eq!(hex::encode_upper_to_slice([0xab, 0x01], &mut out), Ok(4));
    assert_eq!(&out, b"AB01");
}

#[test]
fn encode_to_slice_writes_prefix_of_larger_buffer_only() {
    let mut out = [b'#'; 7];
    assert_eq!(hex::encode_to_slice([0xfe, 0xed], &mut out), Ok(4));
    assert_eq!(&out, b"feed###");
    let mut out = [b'#'; 7];
    assert_eq!(hex::encode_upper_to_slice([0xfe, 0xed], &mut out), Ok(4));
    assert_eq!(&out, b"FEED###");
}

#[test]
fn decode_to_slice_rejects_one_byte_short_and_leaves_output_untouched() {
    let mut out = [0x99u8; 2];
    assert_eq!(
        hex::decode_to_slice("deadbe", &mut out),
        Err(DecodeError::OutputTooSmall { needed: 3 })
    );
    assert_eq!(out, [0x99u8; 2]);

    assert_eq!(
        hex::decode_to_slice("00", &mut []),
        Err(DecodeError::OutputTooSmall { needed: 1 })
    );
}

#[test]
fn decode_to_slice_leaves_output_untouched_on_odd_length() {
    let mut out = [0x99u8; 4];
    assert_eq!(
        hex::decode_to_slice("abc", &mut out),
        Err(DecodeError::OddLength { len: 3 })
    );
    assert_eq!(out, [0x99u8; 4]);
}

#[test]
fn decode_to_slice_fills_exact_buffer() {
    let mut out = [0u8; 2];
    assert_eq!(hex::decode_to_slice("DeAd", &mut out), Ok(2));
    assert_eq!(out, [0xde, 0xad]);
}

#[test]
fn decode_to_slice_writes_prefix_of_larger_buffer_only() {
    let mut out = [0x99u8; 5];
    assert_eq!(hex::decode_to_slice("0102", &mut out), Ok(2));
    assert_eq!(out, [0x01, 0x02, 0x99, 0x99, 0x99]);
}

#[test]
fn decode_to_slice_reports_invalid_byte_with_big_enough_buffer() {
    let mut out = [0u8; 4];
    assert_eq!(
        hex::decode_to_slice("0011zz33", &mut out),
        Err(DecodeError::InvalidByte {
            index: 4,
            byte: b'z'
        })
    );
}

// --- encoded_len ---------------------------------------------------------------

#[test]
fn encoded_len_doubles_up_to_the_largest_valid_input() {
    assert_eq!(hex::encoded_len(1), 2);
    assert_eq!(hex::encoded_len(1 << 20), 2 << 20);
    assert_eq!(hex::encoded_len(usize::MAX / 2), usize::MAX - 1);
}

#[test]
#[should_panic(expected = "overflows usize")]
fn encoded_len_panics_on_overflow_instead_of_wrapping() {
    // Before the 2026-11-24 audit this wrapped to 0 in release builds.
    let _ = hex::encoded_len(usize::MAX / 2 + 1);
}

#[test]
fn encoded_len_is_usable_in_const_context() {
    const LEN: usize = hex::encoded_len(16);
    let buf = [0u8; LEN];
    assert_eq!(buf.len(), 32);
}

// --- Large input ---------------------------------------------------------------

#[test]
fn round_trips_one_mebibyte_of_pseudo_random_bytes() {
    let data = pseudo_random_bytes(1 << 20);
    let lower = hex::encode(&data);
    let upper = hex::encode_upper(&data);
    assert_eq!(lower.len(), 2 << 20);
    assert_eq!(hex::decode(&lower).unwrap(), data);
    assert_eq!(hex::decode(&upper).unwrap(), data);

    let mut buf = vec![0u8; data.len()];
    assert_eq!(hex::decode_to_slice(&upper, &mut buf), Ok(data.len()));
    assert_eq!(buf, data);
}

#[test]
fn reports_invalid_last_byte_of_large_input_with_exact_index() {
    let mut text = "ab".repeat(1 << 19).into_bytes();
    let last = text.len() - 1;
    text[last] = b'!';
    assert_eq!(hex::decode(&text), invalid(last, b'!'));

    text.push(b'0'); // now odd: the invalid byte must still win
    assert_eq!(hex::decode(&text), invalid(last, b'!'));
}

#[test]
fn reports_odd_length_of_large_input() {
    let text = "f".repeat((1 << 20) + 1);
    assert_eq!(
        hex::decode(&text),
        Err(DecodeError::OddLength { len: (1 << 20) + 1 })
    );
}

// --- Error types ---------------------------------------------------------------

#[test]
fn error_display_messages_are_stable() {
    assert_eq!(
        DecodeError::InvalidByte {
            index: 7,
            byte: 0x0a
        }
        .to_string(),
        "invalid hex byte 0x0a at index 7"
    );
    assert_eq!(
        DecodeError::OddLength { len: 3 }.to_string(),
        "odd hex input length 3"
    );
    assert_eq!(
        DecodeError::OutputTooSmall { needed: 2 }.to_string(),
        "output buffer too small: needed 2 bytes"
    );
    assert_eq!(
        BufferTooSmall { needed: 4 }.to_string(),
        "output buffer too small: needed 4 bytes"
    );
}

#[test]
fn errors_work_as_boxed_std_errors() {
    fn run() -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        let mut out = [0u8; 0];
        hex::encode_to_slice(b"x", &mut out)?;
        Ok(hex::decode("zz")?)
    }
    let err = run().unwrap_err();
    assert!(err.downcast_ref::<BufferTooSmall>().is_some());

    let err: Box<dyn std::error::Error> = Box::new(hex::decode("zz").unwrap_err());
    assert_eq!(err.to_string(), "invalid hex byte 0x7a at index 0");
}
