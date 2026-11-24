use akku::hex::{self, BufferTooSmall, DecodeError};

#[test]
fn rfc4648_section_10_vectors() {
    let vectors: &[(&[u8], &str)] = &[
        (b"", ""),
        (b"f", "66"),
        (b"fo", "666f"),
        (b"foo", "666f6f"),
        (b"foob", "666f6f62"),
        (b"fooba", "666f6f6261"),
        (b"foobar", "666f6f626172"),
    ];

    for &(plain, expected_lower) in vectors {
        let expected_upper = expected_lower.to_ascii_uppercase();
        assert_eq!(hex::encoded_len(plain.len()), expected_lower.len());
        assert_eq!(hex::encode(plain), expected_lower);
        assert_eq!(hex::encode_upper(plain), expected_upper);
        assert_eq!(hex::decode(expected_lower).unwrap(), plain);
        assert_eq!(hex::decode(expected_upper).unwrap(), plain);
    }
}

#[test]
fn all_256_bytes_roundtrip() {
    let all_bytes: Vec<u8> = (0u8..=255).collect();
    let lower = hex::encode(&all_bytes);
    let upper = hex::encode_upper(&all_bytes);

    assert_eq!(lower.len(), 512);
    assert_eq!(upper.len(), 512);
    assert_eq!(lower.to_ascii_uppercase(), upper);
    assert_eq!(hex::decode(&lower).unwrap(), all_bytes);
    assert_eq!(hex::decode(&upper).unwrap(), all_bytes);
}

#[test]
fn mixed_case_decoding() {
    assert_eq!(hex::decode("DeAdBeEf").unwrap(), [0xde, 0xad, 0xbe, 0xef]);
}

#[test]
fn error_position_precedence_table() {
    assert_eq!(hex::decode("abc"), Err(DecodeError::OddLength { len: 3 }));
    assert_eq!(
        hex::decode("abg"),
        Err(DecodeError::InvalidByte {
            index: 2,
            byte: b'g',
        })
    );
    assert_eq!(
        hex::decode("a?cde"),
        Err(DecodeError::InvalidByte {
            index: 1,
            byte: b'?',
        })
    );
    assert_eq!(
        hex::decode("0x00"),
        Err(DecodeError::InvalidByte {
            index: 1,
            byte: b'x',
        })
    );
    assert_eq!(
        hex::decode("é"),
        Err(DecodeError::InvalidByte {
            index: 0,
            byte: 0xc3,
        })
    );
    assert_eq!(
        hex::decode(" 00"),
        Err(DecodeError::InvalidByte {
            index: 0,
            byte: b' ',
        })
    );
}

#[test]
fn slice_encoding_and_decoding() {
    let mut enc_buf = [0u8; 3];
    assert_eq!(
        hex::encode_to_slice(b"hi", &mut enc_buf),
        Err(BufferTooSmall { needed: 4 })
    );
    assert_eq!(
        hex::encode_upper_to_slice(b"hi", &mut enc_buf),
        Err(BufferTooSmall { needed: 4 })
    );

    let mut enc_exact = [0u8; 4];
    assert_eq!(hex::encode_to_slice(b"hi", &mut enc_exact), Ok(4));
    assert_eq!(&enc_exact, b"6869");
    assert_eq!(hex::encode_upper_to_slice(b"hi", &mut enc_exact), Ok(4));
    assert_eq!(&enc_exact, b"6869");

    let mut dec_small = [0x99u8; 1];
    assert_eq!(
        hex::decode_to_slice("dead", &mut dec_small),
        Err(DecodeError::OutputTooSmall { needed: 2 })
    );
    assert_eq!(dec_small, [0x99u8; 1]);

    // Invalid byte wins over OutputTooSmall
    assert_eq!(
        hex::decode_to_slice("deaz", &mut dec_small),
        Err(DecodeError::InvalidByte {
            index: 3,
            byte: b'z',
        })
    );

    let mut dec_exact = [0u8; 2];
    assert_eq!(hex::decode_to_slice("DeAd", &mut dec_exact), Ok(2));
    assert_eq!(dec_exact, [0xde, 0xad]);
}
