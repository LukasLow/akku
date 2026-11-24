use akku::base64::{
    self, Alphabet, AlphabetError, BufferTooSmall, DecodeError, DecodePadding, Engine,
};

#[test]
fn rfc4648_section_10_vectors_all_engines() {
    let vectors: &[(&[u8], &str)] = &[
        (b"", ""),
        (b"f", "Zg=="),
        (b"fo", "Zm8="),
        (b"foo", "Zm9v"),
        (b"foob", "Zm9vYg=="),
        (b"fooba", "Zm9vYmE="),
        (b"foobar", "Zm9vYmFy"),
    ];

    for &(plain, padded) in vectors {
        let unpadded = padded.trim_end_matches('=');

        assert_eq!(Engine::STANDARD.encode(plain), padded);
        assert_eq!(Engine::STANDARD.decode(padded).unwrap(), plain);
        assert_eq!(base64::encode(plain), padded);
        assert_eq!(base64::decode(padded).unwrap(), plain);

        assert_eq!(Engine::STANDARD_NO_PAD.encode(plain), unpadded);
        assert_eq!(Engine::STANDARD_NO_PAD.decode(unpadded).unwrap(), plain);

        assert_eq!(Engine::URL_SAFE.encode(plain), padded);
        assert_eq!(Engine::URL_SAFE.decode(padded).unwrap(), plain);

        assert_eq!(Engine::URL_SAFE_NO_PAD.encode(plain), unpadded);
        assert_eq!(Engine::URL_SAFE_NO_PAD.decode(unpadded).unwrap(), plain);
        assert_eq!(base64::encode_url(plain), unpadded);
        assert_eq!(base64::decode_url(unpadded).unwrap(), plain);
    }
}

#[test]
fn url_safe_vs_standard_alphabet_symbols() {
    let raw = [0xfb, 0xef, 0xbe, 0xff];
    assert_eq!(Engine::STANDARD.encode(raw), "++++/w==");
    assert_eq!(Engine::URL_SAFE.encode(raw), "----_w==");
    assert_eq!(Engine::URL_SAFE_NO_PAD.encode(raw), "----_w");
    assert_eq!(base64::encode_url(raw), "----_w");
    assert_eq!(base64::decode_url("----_w").unwrap(), raw);

    assert_eq!(
        Engine::STANDARD.decode("----_w=="),
        Err(DecodeError::InvalidByte {
            index: 0,
            byte: b'-',
        })
    );
    assert_eq!(
        Engine::URL_SAFE.decode("++++/w=="),
        Err(DecodeError::InvalidByte {
            index: 0,
            byte: b'+',
        })
    );
}

#[test]
fn exhaustive_trailing_bits_one_byte_quantum() {
    let alpha = Alphabet::STANDARD.as_bytes();
    let mut valid_count = 0usize;
    let mut invalid_count = 0usize;

    for (idx, &sym) in alpha.iter().enumerate() {
        let input = [b'Z', sym, b'=', b'='];
        let strict = Engine::STANDARD.decode(input);
        let relaxed = Engine::STANDARD.allow_trailing_bits(true).decode(input);
        assert!(relaxed.is_ok());

        if idx % 16 == 0 {
            valid_count += 1;
            assert!(
                strict.is_ok(),
                "index {idx} ({}) should be valid",
                sym as char
            );
        } else {
            invalid_count += 1;
            assert_eq!(
                strict,
                Err(DecodeError::InvalidTrailingBits { index: 1 }),
                "index {idx} ({}) should fail trailing bits",
                sym as char
            );
        }
    }

    assert_eq!(valid_count, 4);
    assert_eq!(invalid_count, 60);
}

#[test]
fn exhaustive_trailing_bits_two_byte_quantum() {
    let alpha = Alphabet::STANDARD.as_bytes();
    let mut valid_count = 0usize;
    let mut invalid_count = 0usize;

    for (idx, &sym) in alpha.iter().enumerate() {
        let input = [b'Z', b'm', sym, b'='];
        let strict = Engine::STANDARD.decode(input);
        let relaxed = Engine::STANDARD.allow_trailing_bits(true).decode(input);
        assert!(relaxed.is_ok());

        if idx % 4 == 0 {
            valid_count += 1;
            assert!(
                strict.is_ok(),
                "index {idx} ({}) should be valid",
                sym as char
            );
        } else {
            invalid_count += 1;
            assert_eq!(
                strict,
                Err(DecodeError::InvalidTrailingBits { index: 2 }),
                "index {idx} ({}) should fail trailing bits",
                sym as char
            );
        }
    }

    assert_eq!(valid_count, 16);
    assert_eq!(invalid_count, 48);
}

#[test]
fn decode_padding_modes() {
    let req = Engine::new(Alphabet::STANDARD, true).with_decode_padding(DecodePadding::Required);
    let forb = Engine::new(Alphabet::STANDARD, false).with_decode_padding(DecodePadding::Forbidden);
    let opt = Engine::new(Alphabet::STANDARD, true).with_decode_padding(DecodePadding::Optional);

    assert_eq!(req.decode("Zg==").unwrap(), b"f");
    assert_eq!(req.decode("Zg"), Err(DecodeError::InvalidLength { len: 2 }));
    assert_eq!(
        req.decode("Zm8"),
        Err(DecodeError::InvalidLength { len: 3 })
    );

    assert_eq!(forb.decode("Zg").unwrap(), b"f");
    assert_eq!(forb.decode("Zm8").unwrap(), b"fo");
    assert_eq!(
        forb.decode("Zg=="),
        Err(DecodeError::InvalidPadding { index: 2 })
    );
    assert_eq!(
        forb.decode("Zm8="),
        Err(DecodeError::InvalidPadding { index: 3 })
    );

    assert_eq!(opt.decode("Zg==").unwrap(), b"f");
    assert_eq!(opt.decode("Zg").unwrap(), b"f");
    assert_eq!(opt.decode("Zm8=").unwrap(), b"fo");
    assert_eq!(opt.decode("Zm8").unwrap(), b"fo");
    assert_eq!(
        opt.decode("Zg="),
        Err(DecodeError::InvalidPadding { index: 2 })
    );

    // Invalid lengths & misplaced padding
    for engine in [req, forb, opt] {
        assert_eq!(
            engine.decode("A"),
            Err(DecodeError::InvalidLength { len: 1 })
        );
        assert_eq!(
            engine.decode("AAAAA"),
            Err(DecodeError::InvalidLength { len: 5 })
        );
        assert_eq!(
            engine.decode("===="),
            Err(DecodeError::InvalidPadding { index: 0 })
        );
        assert_eq!(
            engine.decode("A==="),
            Err(DecodeError::InvalidPadding { index: 1 })
        );
        assert_eq!(
            engine.decode("AA=A"),
            Err(DecodeError::InvalidPadding { index: 2 })
        );
        assert_eq!(
            engine.decode("AAAA=AAA"),
            Err(DecodeError::InvalidPadding { index: 4 })
        );
    }
}

#[test]
fn atomic_decode_into_rollback_and_encode_into() {
    let mut s = String::from("prefix:");
    let written = base64::encode_into(b"hello", &mut s);
    assert_eq!(written, 8);
    assert_eq!(s, "prefix:aGVsbG8=");

    let mut out = vec![0xAA, 0xBB];
    let n = base64::decode_into("aGVsbG8=", &mut out).unwrap();
    assert_eq!(n, 5);
    assert_eq!(out, b"\xAA\xBBhello");

    // Multi-quad input where the error is in the second quad: out must roll back to original len
    let err = base64::decode_into("Zm9vZm!v", &mut out).unwrap_err();
    assert_eq!(
        err,
        DecodeError::InvalidByte {
            index: 6,
            byte: b'!',
        }
    );
    assert_eq!(out, b"\xAA\xBBhello");
}

#[test]
fn encode_and_decode_slice_errors() {
    let mut buf = [0u8; 3];
    assert_eq!(
        Engine::STANDARD.encode_slice(b"fo", &mut buf),
        Err(BufferTooSmall { needed: 4 })
    );
    let mut exact = [0u8; 4];
    assert_eq!(Engine::STANDARD.encode_slice(b"fo", &mut exact), Ok(4));
    assert_eq!(&exact, b"Zm8=");

    let mut out = [0x55u8; 4];
    assert_eq!(
        Engine::STANDARD.decode_slice("aGVsbG8=", &mut out),
        Err(DecodeError::OutputTooSmall { needed: 5 })
    );
    // Output buffer must remain untouched on error
    assert_eq!(out, [0x55u8; 4]);

    let mut out_exact = [0u8; 5];
    assert_eq!(
        Engine::STANDARD.decode_slice("aGVsbG8=", &mut out_exact),
        Ok(5)
    );
    assert_eq!(&out_exact, b"hello");
}

#[test]
fn prng_roundtrip_lengths_0_to_300() {
    let mut state: u64 = 0x1234_5678_9abc_def0;
    let mut next_byte = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 32) as u8
    };

    let engines = [
        Engine::STANDARD,
        Engine::STANDARD_NO_PAD,
        Engine::URL_SAFE,
        Engine::URL_SAFE_NO_PAD,
    ];

    for len in 0..=300 {
        let data: Vec<u8> = (0..len).map(|_| next_byte()).collect();
        for engine in engines {
            let encoded = engine.encode(&data);
            assert_eq!(encoded.len(), engine.encoded_len(len));
            let decoded = engine.decode(&encoded).unwrap();
            assert_eq!(decoded, data);
        }
    }
}

#[test]
fn custom_alphabet_validation_and_roundtrip() {
    let mut with_pad = *Alphabet::STANDARD.as_bytes();
    with_pad[10] = b'=';
    assert_eq!(Alphabet::new(&with_pad), Err(AlphabetError::PaddingByte));

    let mut with_space = *Alphabet::STANDARD.as_bytes();
    with_space[0] = b' ';
    assert_eq!(
        Alphabet::new(&with_space),
        Err(AlphabetError::InvalidByte(b' '))
    );

    let mut with_non_ascii = *Alphabet::STANDARD.as_bytes();
    with_non_ascii[63] = 0x80;
    assert_eq!(
        Alphabet::new(&with_non_ascii),
        Err(AlphabetError::InvalidByte(0x80))
    );

    let mut with_dup = *Alphabet::STANDARD.as_bytes();
    with_dup[5] = with_dup[0];
    assert_eq!(
        Alphabet::new(&with_dup),
        Err(AlphabetError::Duplicate(b'A'))
    );

    let mut reversed = *Alphabet::STANDARD.as_bytes();
    reversed.reverse();
    let custom = Alphabet::new(&reversed).unwrap();
    let engine = Engine::new(custom, true);
    assert_eq!(engine.alphabet().as_bytes(), &reversed);
    let enc = engine.encode(b"akku custom alphabet");
    assert_eq!(engine.decode(&enc).unwrap(), b"akku custom alphabet");
}
